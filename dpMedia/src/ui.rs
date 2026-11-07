//! egui front-end: a compact, translucent "now playing" pill with cover art and
//! a seekable timeline along its lower edge.
//!
//! Rendering is purely reactive: `update` only runs when egui receives an input
//! event or someone calls `request_repaint*`. The media worker and the tray
//! handlers do that on state changes (while a track plays, the worker's once-
//! per-second position update moves the timeline), and the position tracker
//! asks for exactly one delayed repaint to flush the debounced save. Paused,
//! idle or hidden, the overlay is back at zero frames.
//!
//! The window has one fixed size (the pill), so nothing around it can swallow
//! clicks meant for the game and a monitor change cannot leave it resized.
//!
//! "Hidden" (tray toggle) keeps the window alive but paints nothing and makes
//! it click-through. That is free (no frames) and avoids relying on a
//! repaint ever reaching an invisible OS window.
//!
//! Mixed-DPI setups: positions are tracked and stored in physical pixels (they
//! stay valid when the window crosses to a monitor with another scale), and the
//! window size is pinned again whenever the scale factor changes.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CursorIcon, FontId, Painter, Pos2, Rect, RichText, Rounding,
    Sense, Shape, Stroke, TextureHandle, TextureOptions, Ui, Vec2, ViewportBuilder, ViewportCommand,
    ViewportId, WindowLevel,
};

use crate::config::{Config, DEFAULT_X, DEFAULT_Y, MAX_HIDDEN};
use crate::media::{
    display_name, is_browser, ArtUpdate, MediaCommand, MediaHandle, MediaState, SourceInfo,
};
use crate::tray::{Tray, TrayAction};

// --- Pill geometry ------------------------------------------------------------

pub const WINDOW_W: f32 = 380.0;
pub const PILL_H: f32 = 80.0;

const RADIUS: f32 = 24.0;
const PAD_X: f32 = 14.0;
const ART: f32 = 52.0;
const ART_RADIUS: f32 = 12.0;
const BTN: f32 = 32.0;
const BTN_GAP: f32 = 2.0;
const CONTROLS_W: f32 = BTN * 3.0 + BTN_GAP * 2.0;
/// Vertical center of title / artwork / buttons, measured from the pill top.
/// Sits above the geometric middle to leave room for the timeline.
const CONTENT_CY: f32 = 36.0;
const TEXT_X: f32 = PAD_X + ART + 12.0;
const TEXT_MAX_W: f32 = WINDOW_W - 12.0 - CONTROLS_W - 8.0 - TEXT_X;

// --- Timeline geometry ------------------------------------------------------------

/// Horizontal inset of the bar from the pill edges (clear of the rounded corners).
const TL_INSET: f32 = 18.0;
/// Height of the clickable strip along the bottom edge.
const TL_HIT_H: f32 = 18.0;
/// Distance of the bar's center from the bottom edge.
const TL_BAR_FROM_BOTTOM: f32 = 9.0;

/// Wait this long after the last movement before writing the config file.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
/// Movement below this many physical pixels is not a move.
const MOVE_EPSILON_PX: f32 = 1.0;

const TEXT_PRIMARY: Color32 = Color32::from_rgb(245, 245, 247);
const TEXT_SECONDARY: Color32 = Color32::from_rgb(152, 152, 157);
const ICON_DISABLED: Color32 = Color32::from_rgb(99, 99, 102);

// Embedded so every binary carries the license texts it is distributed under.
const LICENSE_MIT: &str = include_str!("../LICENSE-MIT");
const LICENSE_APACHE: &str = include_str!("../LICENSE-APACHE");

const THIRD_PARTY: &[(&str, &str)] = &[
    ("egui / eframe", "MIT OR Apache-2.0"),
    ("image", "MIT OR Apache-2.0"),
    ("serde, serde_json", "MIT OR Apache-2.0"),
    ("dirs", "MIT OR Apache-2.0"),
    ("open", "MIT"),
    ("tray-icon (Windows)", "MIT OR Apache-2.0"),
    ("windows (Windows)", "MIT OR Apache-2.0"),
    ("mpris (Linux)", "Apache-2.0"),
    ("ureq (Linux)", "MIT OR Apache-2.0"),
];

fn panel_fill() -> Color32 {
    // Apple "system gray 6" dark, slightly translucent.
    Color32::from_rgba_unmultiplied(28, 28, 30, 225)
}

fn title_font() -> FontId {
    FontId::proportional(14.5)
}

fn artist_font() -> FontId {
    FontId::proportional(12.5)
}

pub struct DpMedia {
    media: MediaHandle,
    tray: Option<Tray>,
    cfg: Config,
    state: MediaState,
    /// When `state` (and with it the timeline position) was received; the bar
    /// is extrapolated from here while playing.
    state_at: Instant,
    /// Decoded once per track change by the media worker; uploaded here.
    art: Option<TextureHandle>,
    /// Pre-ellipsized strings, rebuilt when the media state or scale changes.
    title_text: String,
    artist_text: String,
    text_dirty: bool,
    /// Last seen window position in *physical* pixels.
    last_pos: Option<Pos2>,
    moved_at: Option<Instant>,
    /// Scale factor of the monitor the window is currently on.
    last_ppp: f32,
    /// The saved position is re-applied once, in the first frame, with the
    /// scale factor of the monitor the window really opened on.
    restored: bool,
    /// False = "hidden": nothing is painted and the window is click-through.
    visible: bool,
    /// Click-through state last sent to the OS (avoids redundant commands).
    passthrough: bool,
    /// Fraction (0..1) the user is dragging the timeline handle to.
    scrub: Option<f32>,
    show_about: bool,
}

impl DpMedia {
    pub fn new(ctx: &egui::Context, media: MediaHandle, tray: Option<Tray>, cfg: Config) -> Self {
        ctx.set_visuals(egui::Visuals::dark());
        Self {
            media,
            tray,
            cfg,
            state: MediaState::default(),
            state_at: Instant::now(),
            art: None,
            title_text: String::new(),
            artist_text: String::new(),
            text_dirty: true,
            last_pos: None,
            moved_at: None,
            last_ppp: ctx.pixels_per_point(),
            restored: false,
            visible: true,
            passthrough: false,
            scrub: None,
            show_about: false,
        }
    }

    // --- State plumbing --------------------------------------------------------

    fn apply_media_update(&mut self, ctx: &egui::Context) {
        if let Some(update) = self.media.latest() {
            if update.state.title != self.state.title || update.state.source != self.state.source {
                self.scrub = None; // a new track invalidates a half-finished drag
            }
            self.state = update.state;
            self.state_at = Instant::now();
            self.text_dirty = true;
            match update.art {
                ArtUpdate::Unchanged => {}
                ArtUpdate::Clear => self.art = None,
                ArtUpdate::Set(image) => {
                    self.art = Some(ctx.load_texture("dpmedia-art", image, TextureOptions::LINEAR));
                }
            }
        }
        if self.text_dirty {
            self.refresh_text(ctx);
            self.text_dirty = false;
        }
    }

    fn refresh_text(&mut self, ctx: &egui::Context) {
        let (title, artist) = if !self.state.has_session {
            ("Nothing playing".to_owned(), String::new())
        } else if self.state.title.is_empty() {
            ("Unknown title".to_owned(), self.state.artist.clone())
        } else {
            (self.state.title.clone(), self.state.artist.clone())
        };
        self.title_text = ellipsize(ctx, &title, &title_font(), TEXT_MAX_W);
        self.artist_text = ellipsize(ctx, &artist, &artist_font(), TEXT_MAX_W);
    }

    fn apply_tray_actions(&mut self, ctx: &egui::Context) {
        while let Some(action) = self.tray.as_ref().and_then(|t| t.try_recv()) {
            match action {
                TrayAction::Toggle => self.set_visible(ctx, !self.visible),
                TrayAction::ResetPosition => self.reset_position(ctx),
                TrayAction::About => {
                    self.show_about = true;
                    ctx.request_repaint();
                }
                TrayAction::Quit => {
                    self.flush_position();
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }

    fn set_visible(&mut self, ctx: &egui::Context, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            // Games can steal the topmost slot; claim it back (without focus).
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop));
        } else {
            self.scrub = None;
        }
        ctx.request_repaint();
    }

    fn apply_visibility(&mut self, ctx: &egui::Context) {
        let want = !self.visible;
        if want != self.passthrough {
            self.passthrough = want;
            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(want));
        }
    }

    // --- Position and monitor changes --------------------------------------------

    /// When the window crosses to a monitor with a different scale factor the OS
    /// may hand it a new size. Pin it back and re-measure the text.
    fn handle_scale_change(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        if (ppp - self.last_ppp).abs() > 0.001 {
            self.last_ppp = ppp;
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(WINDOW_W, PILL_H)));
            self.text_dirty = true;
            ctx.request_repaint();
        }
    }

    /// Re-apply the saved position once the window exists. The position is
    /// stored in physical pixels; dividing by the scale factor of the monitor
    /// the window currently sits on yields exactly that spot, whatever monitor
    /// it lands on.
    fn restore_position(&mut self, ctx: &egui::Context) {
        if self.restored {
            return;
        }
        self.restored = true;

        let (Some(px), Some(py)) = (self.cfg.phys_x, self.cfg.phys_y) else {
            return; // older config: the logical position from startup stands
        };
        let ppp = ctx.pixels_per_point();
        if ppp <= 0.0 {
            return;
        }
        let size = vec2(WINDOW_W * ppp, PILL_H * ppp);
        let phys = clamp_to_virtual_screen(pos2(px, py), size);
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(phys.x / ppp, phys.y / ppp)));
    }

    /// Center the pill on the primary display and make sure it is shown.
    fn reset_position(&mut self, ctx: &egui::Context) {
        let target = match primary_screen_logical(ctx) {
            Some(screen) => pos2(
                ((screen.x - WINDOW_W) / 2.0).max(0.0),
                ((screen.y - PILL_H) / 2.0).max(0.0),
            ),
            None => pos2(DEFAULT_X, DEFAULT_Y),
        };
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(target));
        self.set_visible(ctx, true);
        ctx.request_repaint();
    }

    /// Ctrl + left mouse button hands the window to the OS move loop.
    fn handle_drag(&self, ctx: &egui::Context) {
        let (ctrl, pressed) = ctx.input(|i| (i.modifiers.ctrl, i.pointer.primary_pressed()));
        if ctrl {
            ctx.set_cursor_icon(CursorIcon::Grab);
            if pressed {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
        }
    }

    /// Notice window moves (the OS drag loop gives us no "drag ended" event)
    /// and persist the final position once it has been still for a moment.
    /// Positions are compared in physical pixels, which stay continuous when the
    /// window crosses to a monitor with a different scale factor.
    fn track_position(&mut self, ctx: &egui::Context) {
        let Some(rect) = ctx.input(|i| i.viewport().outer_rect) else {
            return; // e.g. Wayland: the compositor never tells us where we are
        };
        let ppp = ctx.pixels_per_point();
        let phys = pos2(rect.min.x * ppp, rect.min.y * ppp);

        match self.last_pos {
            None => self.last_pos = Some(phys),
            Some(prev) if (prev - phys).length() > MOVE_EPSILON_PX => {
                self.last_pos = Some(phys);
                self.moved_at = Some(Instant::now());
            }
            _ => {}
        }

        if let Some(at) = self.moved_at {
            let elapsed = at.elapsed();
            if elapsed >= SAVE_DEBOUNCE {
                self.moved_at = None;
                self.save_position(phys);
            } else {
                ctx.request_repaint_after(SAVE_DEBOUNCE - elapsed);
            }
        }
    }

    /// Write any not-yet-saved move immediately (quit / close request).
    fn flush_position(&mut self) {
        if self.moved_at.take().is_some() {
            if let Some(phys) = self.last_pos {
                self.save_position(phys);
            }
        }
    }

    fn save_position(&mut self, phys: Pos2) {
        self.cfg.phys_x = Some(phys.x);
        self.cfg.phys_y = Some(phys.y);
        // Logical copy for older versions of the config reader.
        let ppp = self.last_ppp.max(0.1);
        self.cfg.x = phys.x / ppp;
        self.cfg.y = phys.y / ppp;
        self.save_config();
    }

    fn save_config(&self) {
        if let Err(err) = self.cfg.save() {
            eprintln!("dpmedia: could not save config: {err}");
        }
    }

    // --- Timeline state ------------------------------------------------------------

    /// Playback position now: the last reported one plus the time since, if playing.
    fn current_position_ms(&self) -> u64 {
        let timeline = self.state.timeline;
        let mut position = timeline.position_ms;
        if self.state.playing {
            position += self.state_at.elapsed().as_millis() as u64;
        }
        position.min(timeline.duration_ms)
    }

    fn seek_to_fraction(&mut self, ctx: &egui::Context, fraction: f32) {
        let duration = self.state.timeline.duration_ms;
        let target = (f64::from(fraction.clamp(0.0, 1.0)) * duration as f64) as u64;
        // Show the jump immediately; the worker confirms with a fresh poll.
        self.state.timeline.position_ms = target;
        self.state_at = Instant::now();
        self.media.send(MediaCommand::Seek(target));
        ctx.request_repaint();
    }

    // --- Painting --------------------------------------------------------------

    fn pill(&mut self, ui: &Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        let ctrl = ui.input(|i| i.modifiers.ctrl);
        let hovered = ui.rect_contains_pointer(rect);
        let painter = ui.painter();
        // While Ctrl is held the whole window is a drag handle, not a button.
        let live = !ctrl;

        // Translucent rounded card + hairline border.
        painter.rect_filled(rect, Rounding::same(RADIUS), panel_fill());
        painter.rect_stroke(
            rect.shrink(0.5),
            Rounding::same(RADIUS),
            Stroke::new(1.0, Color32::from_white_alpha(26)),
        );

        let cy = rect.min.y + CONTENT_CY;
        let controls_left = rect.max.x - 12.0 - CONTROLS_W;

        // --- Artwork ---------------------------------------------------------
        let art_rect = Rect::from_min_size(pos2(rect.min.x + PAD_X, cy - ART / 2.0), Vec2::splat(ART));
        self.paint_art(ui, art_rect);

        // --- Title / artist --------------------------------------------------
        let text_x = rect.min.x + TEXT_X;
        let title_color = if self.state.has_session { TEXT_PRIMARY } else { TEXT_SECONDARY };
        if self.artist_text.is_empty() {
            painter.text(pos2(text_x, cy), Align2::LEFT_CENTER, &self.title_text, title_font(), title_color);
        } else {
            painter.text(pos2(text_x, cy - 9.0), Align2::LEFT_CENTER, &self.title_text, title_font(), title_color);
            painter.text(pos2(text_x, cy + 10.0), Align2::LEFT_CENTER, &self.artist_text, artist_font(), TEXT_SECONDARY);
        }

        // --- Transport buttons -----------------------------------------------
        let middle = if self.state.playing { Icon::Pause } else { Icon::Play };
        let buttons = [
            (Icon::Previous, MediaCommand::Previous),
            (middle, MediaCommand::PlayPause),
            (Icon::Next, MediaCommand::Next),
        ];
        let enabled = self.state.has_session;

        for (i, (icon, cmd)) in buttons.into_iter().enumerate() {
            let center = pos2(controls_left + BTN / 2.0 + i as f32 * (BTN + BTN_GAP), cy);
            let btn_rect = Rect::from_center_size(center, Vec2::splat(BTN));
            let resp = ui.interact(btn_rect, ui.id().with(("dpmedia-btn", i)), Sense::click());

            let active = enabled && live;
            if active && resp.hovered() {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
                let alpha = if resp.is_pointer_button_down_on() { 36 } else { 20 };
                painter.circle_filled(center, BTN / 2.0, Color32::from_white_alpha(alpha));
            }

            let color = if enabled { TEXT_PRIMARY } else { ICON_DISABLED };
            draw_icon(painter, center, icon, color);

            if active && resp.clicked() {
                self.media.send(cmd);
            }
        }

        // --- Timeline along the lower edge ---------------------------------------
        self.timeline_bar(ui, rect, live);

        // --- Info "i" (about + settings) and close "x", only while hovering ----
        if hovered {
            let close_rect = Rect::from_center_size(pos2(rect.max.x - 15.0, rect.min.y + 13.0), Vec2::splat(16.0));
            let info_rect = Rect::from_center_size(pos2(rect.max.x - 35.0, rect.min.y + 13.0), Vec2::splat(16.0));

            let info = ui.interact(info_rect, ui.id().with("dpmedia-info"), Sense::click());
            let alpha = if info.hovered() { 210 } else { 80 };
            let stroke = Stroke::new(1.2, Color32::from_white_alpha(alpha));
            let c = info_rect.center();
            painter.circle_stroke(c, 6.0, stroke);
            painter.circle_filled(c + vec2(0.0, -2.6), 0.9, stroke.color);
            painter.line_segment([c + vec2(0.0, -0.6), c + vec2(0.0, 3.0)], stroke);
            if live && info.clicked() {
                self.show_about = true;
                ctx.request_repaint();
            }

            let close = ui.interact(close_rect, ui.id().with("dpmedia-close"), Sense::click());
            let alpha = if close.hovered() { 210 } else { 80 };
            let stroke = Stroke::new(1.4, Color32::from_white_alpha(alpha));
            let c = close_rect.center();
            let d = 3.5;
            painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
            painter.line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
            if live && close.clicked() {
                self.flush_position();
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }

    /// Always-visible progress bar along the bottom edge. Click anywhere on it to
    /// jump there, or drag the handle to scrub. Hovering shows the time a click
    /// would jump to and the total length.
    fn timeline_bar(&mut self, ui: &Ui, rect: Rect, live: bool) {
        let ctx = ui.ctx().clone();
        let painter = ui.painter();

        let timeline = self.state.timeline;
        let has_timeline = self.state.has_session && timeline.duration_ms > 0;
        let seekable = has_timeline && timeline.can_seek;
        let active = live && seekable;

        let left = rect.min.x + TL_INSET;
        let right = rect.max.x - TL_INSET;
        let bar_y = rect.max.y - TL_BAR_FROM_BOTTOM;
        let hit = Rect::from_min_max(
            pos2(rect.min.x + 10.0, rect.max.y - TL_HIT_H),
            pos2(rect.max.x - 10.0, rect.max.y),
        );
        let sense = if seekable { Sense::click_and_drag() } else { Sense::hover() };
        let resp = ui.interact(hit, ui.id().with("dpmedia-seek"), sense);

        let pointer_fraction = ui
            .input(|i| i.pointer.interact_pos())
            .map(|p| ((p.x - left) / (right - left)).clamp(0.0, 1.0));

        if active {
            if resp.dragged() {
                if let Some(fraction) = pointer_fraction {
                    self.scrub = Some(fraction);
                }
            }
            if resp.drag_stopped() {
                if let Some(fraction) = self.scrub.take() {
                    self.seek_to_fraction(&ctx, fraction);
                }
            } else if resp.clicked() {
                if let Some(fraction) = pointer_fraction {
                    self.seek_to_fraction(&ctx, fraction);
                }
            }
        }
        if !resp.dragged() {
            self.scrub = None;
        }

        let dragging = self.scrub.is_some();
        let hot = active && (resp.hovered() || dragging);
        if hot {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
        }

        let fraction = if has_timeline {
            self.scrub
                .unwrap_or_else(|| self.current_position_ms() as f32 / timeline.duration_ms as f32)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };

        // --- Bar ---------------------------------------------------------------
        let thickness = if hot { 6.0 } else { 4.0 };
        let track = Rect::from_min_max(pos2(left, bar_y - thickness / 2.0), pos2(right, bar_y + thickness / 2.0));
        painter.rect_filled(track, Rounding::same(thickness / 2.0), Color32::from_white_alpha(38));
        if has_timeline {
            let x = left + (right - left) * fraction;
            let filled = Rect::from_min_max(track.min, pos2(x, track.max.y));
            let color = if seekable { TEXT_PRIMARY } else { TEXT_SECONDARY };
            painter.rect_filled(filled, Rounding::same(thickness / 2.0), color);
            if hot {
                painter.circle_filled(pos2(x, bar_y), 5.5, TEXT_PRIMARY);
            }
        }

        // --- Time bubble while hovering / scrubbing ------------------------------------
        if hot && has_timeline {
            let duration = timeline.duration_ms;
            let at = self.scrub.or(pointer_fraction).unwrap_or(fraction);
            let text = format!(
                "{} / {}",
                format_time((f64::from(at) * duration as f64) as u64),
                format_time(duration)
            );
            let galley = painter.layout_no_wrap(text, FontId::proportional(11.0), TEXT_PRIMARY);
            let size = galley.size() + vec2(14.0, 6.0);
            let center_x = (left + (right - left) * at)
                .clamp(rect.min.x + size.x / 2.0 + 6.0, rect.max.x - size.x / 2.0 - 6.0);
            let chip = Rect::from_center_size(pos2(center_x, bar_y - 17.0), size);
            painter.rect_filled(chip, Rounding::same(8.0), Color32::from_rgba_unmultiplied(18, 18, 20, 245));
            painter.rect_stroke(chip, Rounding::same(8.0), Stroke::new(1.0, Color32::from_white_alpha(30)));
            painter.galley(chip.center() - galley.size() / 2.0, galley, TEXT_PRIMARY);
        }
    }

    /// Rounded-square cover (Apple style) or a placeholder note.
    fn paint_art(&self, ui: &Ui, rect: Rect) {
        let rounding = Rounding::same(ART_RADIUS);
        let painter = ui.painter();
        match &self.art {
            Some(texture) => {
                egui::Image::new(texture).rounding(rounding).paint_at(ui, rect);
            }
            None => {
                painter.rect_filled(rect, rounding, Color32::from_rgb(52, 52, 56));
                // Music note drawn from primitives (no font dependency).
                let c = rect.center();
                let color = Color32::from_white_alpha(90);
                painter.circle_filled(c + vec2(-5.0, 9.0), 4.5, color);
                painter.line_segment([c + vec2(-0.8, 8.0), c + vec2(-0.8, -11.0)], Stroke::new(2.0, color));
                painter.line_segment([c + vec2(-0.8, -11.0), c + vec2(8.0, -6.0)], Stroke::new(2.4, color));
            }
        }
        painter.rect_stroke(rect, rounding, Stroke::new(1.0, Color32::from_white_alpha(20)));
    }

    // --- About & settings ----------------------------------------------------------

    /// Separate OS window that exists only while open (no cost otherwise).
    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }

        let mut close = false;
        let mut changed = false;
        let cfg = &mut self.cfg;
        let sources = &self.state.sources;
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("dpmedia-about"),
            ViewportBuilder::default()
                .with_title("About & Settings \u{2013} dpMedia")
                .with_inner_size([480.0, 640.0])
                .with_always_on_top()
                .with_taskbar(false),
            |ctx, _class| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        changed |= about_contents(ui, cfg, sources);
                    });
                });
                if ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
            },
        );

        if changed {
            self.save_config();
            self.media.set_filter(self.cfg.filter());
        }
        if close {
            self.show_about = false;
            ctx.request_repaint();
        }
    }
}

/// Contents of the About & Settings window. Returns true if a setting changed.
fn about_contents(ui: &mut Ui, cfg: &mut Config, sources: &[SourceInfo]) -> bool {
    let repo = env!("CARGO_PKG_REPOSITORY");
    let mut changed = false;

    ui.heading("dpMedia");
    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
    ui.label("A tiny always-on-top \"now playing\" overlay for Windows and Linux.");
    ui.add_space(6.0);
    ui.label(format!(
        "\u{a9} 2026 {} \u{b7} licensed under MIT OR Apache-2.0",
        env!("CARGO_PKG_AUTHORS")
    ));
    if !repo.is_empty() && ui.link("Source code on GitHub").clicked() {
        open_url(repo); // fixed URL from Cargo.toml, not user input
    }

    // --- Settings ---------------------------------------------------------------
    ui.separator();
    ui.label(RichText::new("Settings").strong());
    ui.label(RichText::new("Media sources").size(12.5).color(TEXT_SECONDARY));
    changed |= ui
        .checkbox(
            &mut cfg.hide_browsers,
            "Ignore browser media (YouTube, Netflix, \u{2026})",
        )
        .changed();
    ui.add_space(4.0);
    ui.label(
        RichText::new("Detected players \u{2013} untick one to ignore it")
            .size(11.5)
            .color(TEXT_SECONDARY),
    );

    // Running players plus ones switched off earlier (so they can be re-enabled).
    let mut entries: Vec<(String, String)> = sources
        .iter()
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();
    for id in &cfg.hidden_sources {
        if !entries.iter().any(|(known, _)| known == id) {
            entries.push((id.clone(), display_name(id)));
        }
    }
    if entries.is_empty() {
        ui.label(
            RichText::new("Nothing is playing right now \u{2013} players show up here while they run.")
                .size(11.5)
                .color(TEXT_SECONDARY),
        );
    }
    for (id, name) in entries {
        let browser = is_browser(&id);
        let off_by_browser_rule = cfg.hide_browsers && browser;
        let off_by_list = cfg.hidden_sources.contains(&id);
        let mut shown = !(off_by_browser_rule || off_by_list);
        let label = if browser { format!("{name} (browser)") } else { name };

        let response = ui.add_enabled(!off_by_browser_rule, egui::Checkbox::new(&mut shown, label));
        if response.changed() {
            if shown {
                cfg.hidden_sources.retain(|hidden| hidden != &id);
            } else if !off_by_list && cfg.hidden_sources.len() < MAX_HIDDEN {
                cfg.hidden_sources.push(id.clone());
            }
            changed = true;
        }
    }

    // --- Legal ------------------------------------------------------------------
    ui.separator();
    ui.label(RichText::new("Haftung / Liability").strong());
    ui.label(
        "Dieses Programm wird unentgeltlich zur Verf\u{fc}gung gestellt (Schenkung, \u{a7}\u{a7} 516 ff. BGB). \
         Nach \u{a7} 521 BGB haftet der Anbieter nur f\u{fc}r Vorsatz und grobe Fahrl\u{e4}ssigkeit. \
         Zwingende gesetzliche Haftung bleibt unber\u{fc}hrt.",
    );
    ui.add_space(4.0);
    ui.label(
        "This program is provided free of charge (a gift under German law). Under \u{a7} 521 BGB the \
         provider is liable only for intent and gross negligence. Mandatory statutory liability \
         remains unaffected.",
    );

    ui.separator();
    ui.label(RichText::new("Third-party software").strong());
    for (name, license) in THIRD_PARTY {
        ui.label(format!("\u{2022} {name} \u{2013} {license}"));
    }
    ui.label(
        "Fonts bundled with egui: Ubuntu-Light (Ubuntu Font Licence 1.0), Hack (MIT / Bitstream \
         Vera), Noto Emoji (SIL OFL 1.1), emoji-icon-font.",
    );
    ui.add_space(4.0);
    ui.label(
        "dpMedia is an independent project and is not affiliated with, endorsed by or sponsored by \
         Spotify AB. \"Spotify\" is a trademark of Spotify AB.",
    );

    ui.separator();
    egui::CollapsingHeader::new("MIT License").show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("mit").max_height(200.0).show(ui, |ui| {
            ui.monospace(LICENSE_MIT);
        });
    });
    egui::CollapsingHeader::new("Apache License 2.0").show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("apache").max_height(200.0).show(ui, |ui| {
            ui.monospace(LICENSE_APACHE);
        });
    });

    changed
}

/// Open a link with the OS default handler (only ever called with a fixed URL).
fn open_url(url: &str) {
    if let Err(err) = open::that_detached(url) {
        eprintln!("dpmedia: could not open {url}: {err}");
    }
}

/// `m:ss`, or `h:mm:ss` for tracks of an hour or more.
fn format_time(ms: u64) -> String {
    let total = ms / 1000;
    let (hours, minutes, seconds) = (total / 3600, (total / 60) % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Logical size of the primary display, used to center the widget.
#[cfg(windows)]
fn primary_screen_logical(ctx: &egui::Context) -> Option<Vec2> {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    // The process is per-monitor DPI aware (winit), so these are physical
    // pixels of the primary monitor. Dividing by the *window's* scale factor
    // matches how winit converts the logical position we send back.
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let ppp = ctx.pixels_per_point();
    (w > 0 && h > 0 && ppp > 0.0).then(|| vec2(w as f32 / ppp, h as f32 / ppp))
}

#[cfg(not(windows))]
fn primary_screen_logical(ctx: &egui::Context) -> Option<Vec2> {
    // Linux cannot name the "primary" monitor portably; use the current one.
    ctx.input(|i| i.viewport().monitor_size)
}

/// Keep a saved physical position on the desktop: a monitor that was unplugged
/// since the last run must not leave the widget somewhere unreachable.
#[cfg(windows)]
fn clamp_to_virtual_screen(phys: Pos2, size: Vec2) -> Pos2 {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN) as f32,
            GetSystemMetrics(SM_YVIRTUALSCREEN) as f32,
            GetSystemMetrics(SM_CXVIRTUALSCREEN) as f32,
            GetSystemMetrics(SM_CYVIRTUALSCREEN) as f32,
        )
    };
    if w <= 0.0 || h <= 0.0 {
        return phys;
    }
    let max_x = (x + w - size.x).max(x);
    let max_y = (y + h - size.y).max(y);
    pos2(phys.x.clamp(x, max_x), phys.y.clamp(y, max_y))
}

#[cfg(not(windows))]
fn clamp_to_virtual_screen(phys: Pos2, _size: Vec2) -> Pos2 {
    phys
}

impl eframe::App for DpMedia {
    /// Fully transparent framebuffer; the card is painted by us.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_media_update(ctx);
        self.apply_tray_actions(ctx);

        if ctx.input(|i| i.viewport().close_requested()) {
            self.flush_position();
        }

        self.handle_scale_change(ctx);
        self.restore_position(ctx);
        self.apply_visibility(ctx);
        if self.visible {
            self.handle_drag(ctx);
        }
        self.track_position(ctx);

        // The timeline is always on screen, so the worker publishes position
        // ticks (about once per second, only while something plays).
        self.media.set_timeline_wanted(self.visible);

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                if self.visible {
                    let rect = ui.max_rect();
                    self.pill(ui, rect);
                }
            });

        self.about_window(ctx);
    }
}

/// Shorten `text` with "…" so it fits `max_w` (binary search over prefix length).
fn ellipsize(ctx: &egui::Context, text: &str, font: &FontId, max_w: f32) -> String {
    let width = |s: &str| {
        ctx.fonts(|f| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x)
    };
    if width(text) <= max_w {
        return text.to_owned();
    }

    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let candidate: String = chars[..mid].iter().collect::<String>() + "\u{2026}";
        if width(&candidate) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let prefix: String = chars[..lo].iter().collect();
    format!("{}\u{2026}", prefix.trim_end())
}

// ---------------------------------------------------------------------------
// Icons are painted as shapes so we don't depend on any font's glyph coverage.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Previous,
    Play,
    Pause,
    Next,
}

fn draw_icon(p: &Painter, c: Pos2, icon: Icon, color: Color32) {
    match icon {
        Icon::Play => triangle(p, c + vec2(1.0, 0.0), 1.0, 14.0, 17.0, color),
        Icon::Pause => {
            for dx in [-4.75, 4.75] {
                p.rect_filled(
                    Rect::from_center_size(c + vec2(dx, 0.0), vec2(4.5, 16.0)),
                    Rounding::same(1.5),
                    color,
                );
            }
        }
        Icon::Next => {
            triangle(p, c + vec2(-5.0, 0.0), 1.0, 10.0, 13.0, color);
            triangle(p, c + vec2(5.0, 0.0), 1.0, 10.0, 13.0, color);
        }
        Icon::Previous => {
            triangle(p, c + vec2(-5.0, 0.0), -1.0, 10.0, 13.0, color);
            triangle(p, c + vec2(5.0, 0.0), -1.0, 10.0, 13.0, color);
        }
    }
}

/// Solid triangle pointing right (`dir` = 1.0) or left (`dir` = -1.0).
fn triangle(p: &Painter, c: Pos2, dir: f32, w: f32, h: f32, color: Color32) {
    let mut pts = vec![
        pos2(c.x - dir * w * 0.5, c.y - h * 0.5),
        pos2(c.x + dir * w * 0.5, c.y),
        pos2(c.x - dir * w * 0.5, c.y + h * 0.5),
    ];
    if dir < 0.0 {
        pts.reverse(); // keep clockwise winding for clean anti-aliasing
    }
    p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

#[cfg(test)]
mod tests {
    use super::format_time;

    #[test]
    fn formats_times() {
        assert_eq!(format_time(0), "0:00");
        assert_eq!(format_time(61_000), "1:01");
        assert_eq!(format_time(3_599_000), "59:59");
        assert_eq!(format_time(3_725_000), "1:02:05");
    }
}
