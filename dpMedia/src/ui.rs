//! egui front-end: a compact, translucent "now playing" pill.
//!
//! Rendering is purely reactive: `update` only runs when egui receives an
//! input event or someone calls `request_repaint*`. The media worker does that
//! on state changes, and the position tracker below asks for exactly one
//! delayed repaint to flush the debounced save. No continuous animation.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CursorIcon, FontId, Painter, Pos2, Rect, Rounding, Sense,
    Shape, Stroke, Ui, Vec2, ViewportCommand,
};

use crate::config::Config;
use crate::media::{MediaCommand, MediaHandle, MediaState};

pub const WINDOW_W: f32 = 320.0;
pub const WINDOW_H: f32 = 76.0;

const RADIUS: f32 = 22.0;
const PAD_X: f32 = 16.0;
const BTN: f32 = 34.0;
const BTN_GAP: f32 = 2.0;
const CONTROLS_W: f32 = BTN * 3.0 + BTN_GAP * 2.0;
const TEXT_MAX_W: f32 = WINDOW_W - PAD_X * 2.0 - CONTROLS_W - 10.0;

/// Wait this long after the last movement before writing the config file.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
const MOVE_EPSILON: f32 = 0.5;

const TEXT_PRIMARY: Color32 = Color32::from_rgb(245, 245, 247);
const TEXT_SECONDARY: Color32 = Color32::from_rgb(152, 152, 157);
const ICON_DISABLED: Color32 = Color32::from_rgb(99, 99, 102);

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
    state: MediaState,
    /// Pre-ellipsized strings, rebuilt only when the media state changes.
    title_text: String,
    artist_text: String,
    text_dirty: bool,
    last_pos: Option<Pos2>,
    moved_at: Option<Instant>,
}

impl DpMedia {
    pub fn new(media: MediaHandle) -> Self {
        Self {
            media,
            state: MediaState::default(),
            title_text: String::new(),
            artist_text: String::new(),
            text_dirty: true,
            last_pos: None,
            moved_at: None,
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
    fn track_position(&mut self, ctx: &egui::Context) {
        let Some(pos) = ctx.input(|i| i.viewport().outer_rect).map(|r| r.min) else {
            return; // e.g. Wayland: the compositor never tells us where we are
        };

        match self.last_pos {
            None => self.last_pos = Some(pos),
            Some(prev) if (prev - pos).length() > MOVE_EPSILON => {
                self.last_pos = Some(pos);
                self.moved_at = Some(Instant::now());
            }
            _ => {}
        }

        if let Some(at) = self.moved_at {
            let elapsed = at.elapsed();
            if elapsed >= SAVE_DEBOUNCE {
                self.moved_at = None;
                save_position(pos);
            } else {
                ctx.request_repaint_after(SAVE_DEBOUNCE - elapsed);
            }
        }
    }

    /// Write any not-yet-saved move immediately (quit / close request).
    fn flush_position(&mut self) {
        if self.moved_at.take().is_some() {
            if let Some(pos) = self.last_pos {
                save_position(pos);
            }
        }
    }

    fn panel(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let ctrl = ui.input(|i| i.modifiers.ctrl);
        let hovered = ui.rect_contains_pointer(rect);
        let painter = ui.painter();

        // Translucent rounded card + hairline border.
        painter.rect_filled(rect, Rounding::same(RADIUS), panel_fill());
        painter.rect_stroke(
            rect.shrink(0.5),
            Rounding::same(RADIUS),
            Stroke::new(1.0, Color32::from_white_alpha(26)),
        );

        let cy = rect.center().y;
        let text_x = rect.min.x + PAD_X;
        let controls_left = rect.max.x - PAD_X - CONTROLS_W;

        // --- Title / artist -------------------------------------------------
        let title_color = if self.state.has_session { TEXT_PRIMARY } else { TEXT_SECONDARY };
        if self.artist_text.is_empty() {
            painter.text(pos2(text_x, cy), Align2::LEFT_CENTER, &self.title_text, title_font(), title_color);
        } else {
            painter.text(pos2(text_x, cy - 9.0), Align2::LEFT_CENTER, &self.title_text, title_font(), title_color);
            painter.text(pos2(text_x, cy + 10.0), Align2::LEFT_CENTER, &self.artist_text, artist_font(), TEXT_SECONDARY);
        }

        // --- Transport buttons ---------------------------------------------
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

            // While Ctrl is held the whole window is a drag handle, not a button.
            let live = enabled && !ctrl;
            if live && resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                let alpha = if resp.is_pointer_button_down_on() { 36 } else { 20 };
                painter.circle_filled(center, BTN / 2.0, Color32::from_white_alpha(alpha));
            }

            let color = if enabled { TEXT_PRIMARY } else { ICON_DISABLED };
            draw_icon(painter, center, icon, color);

            if live && resp.clicked() {
                self.media.send(cmd);
            }
        }

        // --- Close "x", only while hovering ---------------------------------
        if hovered {
            let x_rect = Rect::from_center_size(pos2(rect.max.x - 15.0, rect.min.y + 12.0), Vec2::splat(16.0));
            let resp = ui.interact(x_rect, ui.id().with("dpmedia-close"), Sense::click());
            let alpha = if resp.hovered() { 210 } else { 80 };
            let stroke = Stroke::new(1.4, Color32::from_white_alpha(alpha));
            let c = x_rect.center();
            let d = 3.5;
            painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
            painter.line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);

            if !ctrl && resp.clicked() {
                self.flush_position();
                ui.ctx().send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }
}

impl eframe::App for DpMedia {
    /// Fully transparent framebuffer; the card is painted by us.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(state) = self.media.latest() {
            self.state = state;
            self.text_dirty = true;
        }
        if self.text_dirty {
            self.refresh_text(ctx);
            self.text_dirty = false;
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            self.flush_position();
        }

        self.handle_drag(ctx);
        self.track_position(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| self.panel(ui));
    }
}

fn save_position(pos: Pos2) {
    if let Err(err) = (Config { x: pos.x, y: pos.y }).save() {
        eprintln!("dpmedia: could not save window position: {err}");
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
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        if width(&candidate) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let prefix: String = chars[..lo].iter().collect();
    format!("{}…", prefix.trim_end())
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
