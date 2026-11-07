//! System tray (notification area) icon with a small context menu.
//!
//! Windows only. On Linux a tray icon would need GTK / libappindicator, which
//! is a heavy dependency for a tiny overlay, so `Tray::new` returns `None`
//! there and the overlay simply stays visible (the hover "i" / "x" buttons in
//! the widget cover About and Quit).
//!
//! Events arrive on the UI thread (the tray's hidden window lives on the winit
//! message loop). The handlers only push a `TrayAction` into a channel and call
//! `Context::request_repaint`, so the app stays at zero frames while idle and
//! wakes up exactly once per tray interaction.

use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum TrayAction {
    /// Show / hide the overlay (also bound to a left click on the icon).
    Toggle,
    /// Center the widget on the primary display.
    ResetPosition,
    /// Open the About / Settings window (info, media sources, licenses).
    About,
    Quit,
}

#[cfg(windows)]
pub struct Tray {
    rx: std::sync::mpsc::Receiver<TrayAction>,
    /// Dropping the icon removes it from the notification area.
    _icon: tray_icon::TrayIcon,
}

#[cfg(windows)]
impl Tray {
    pub fn new(ctx: &egui::Context) -> Option<Self> {
        use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
        use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

        let toggle = MenuItem::new("Toggle Overlay", true, None);
        let reset = MenuItem::new("Reset Position", true, None);
        let about = MenuItem::new("About / Settings", true, None);
        let quit = MenuItem::new("Quit dpMedia", true, None);

        let menu = Menu::new();
        let appended = menu.append_items(&[
            &toggle,
            &reset,
            &PredefinedMenuItem::separator(),
            &about,
            &PredefinedMenuItem::separator(),
            &quit,
        ]);
        if let Err(err) = appended {
            eprintln!("dpmedia: could not build tray menu: {err}");
            return None;
        }

        let (toggle_id, reset_id, about_id, quit_id) = (
            toggle.id().clone(),
            reset.id().clone(),
            about.id().clone(),
            quit.id().clone(),
        );

        let (tx, rx) = std::sync::mpsc::channel::<TrayAction>();

        {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let action = if event.id == toggle_id {
                    TrayAction::Toggle
                } else if event.id == reset_id {
                    TrayAction::ResetPosition
                } else if event.id == about_id {
                    TrayAction::About
                } else if event.id == quit_id {
                    TrayAction::Quit
                } else {
                    return;
                };
                let _ = tx.send(action);
                ctx.request_repaint();
            }));
        }

        {
            let ctx = ctx.clone();
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                // Left click (on release) toggles; right click opens the menu.
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    let _ = tx.send(TrayAction::Toggle);
                    ctx.request_repaint();
                }
            }));
        }

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("dpMedia")
            .with_icon(make_icon()?)
            .build();

        match icon {
            Ok(icon) => Some(Self { rx, _icon: icon }),
            Err(err) => {
                eprintln!("dpmedia: could not create tray icon: {err}");
                None
            }
        }
    }

    /// Next pending tray action, if any (never blocks).
    pub fn try_recv(&self) -> Option<TrayAction> {
        self.rx.try_recv().ok()
    }
}

/// 32x32 icon drawn in code (rounded dark square with a play triangle), so no
/// binary asset has to live in the repository. 3x3 supersampling smooths edges.
#[cfg(windows)]
fn make_icon() -> Option<tray_icon::Icon> {
    const N: u32 = 32;
    const SUB: u32 = 3;
    const BOX_RGB: [f32; 3] = [58.0, 58.0, 62.0];

    // Rounded square: half-size 15, corner radius 7, centered in the bitmap.
    let in_box = |x: f32, y: f32| {
        let (cx, cy) = ((x - 16.0).abs() - 8.0, (y - 16.0).abs() - 8.0);
        cx.max(0.0).hypot(cy.max(0.0)) + cx.max(cy).min(0.0) - 7.0 <= 0.0
    };
    // Play triangle: (12, 9.5) - (12, 22.5) - (23.5, 16).
    let in_tri = |x: f32, y: f32| {
        let edge = |ax: f32, ay: f32, bx: f32, by: f32| (bx - ax) * (y - ay) - (by - ay) * (x - ax);
        let d1 = edge(12.0, 9.5, 12.0, 22.5);
        let d2 = edge(12.0, 22.5, 23.5, 16.0);
        let d3 = edge(23.5, 16.0, 12.0, 9.5);
        (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
    };

    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for py in 0..N {
        for px in 0..N {
            let (mut box_hits, mut tri_hits) = (0u32, 0u32);
            for sy in 0..SUB {
                for sx in 0..SUB {
                    let x = px as f32 + (sx as f32 + 0.5) / SUB as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SUB as f32;
                    if in_box(x, y) {
                        box_hits += 1;
                        if in_tri(x, y) {
                            tri_hits += 1;
                        }
                    }
                }
            }
            let alpha = box_hits as f32 / (SUB * SUB) as f32;
            let white = if box_hits > 0 {
                tri_hits as f32 / box_hits as f32
            } else {
                0.0
            };
            for c in BOX_RGB {
                rgba.push((c * (1.0 - white) + 255.0 * white).round() as u8);
            }
            rgba.push((alpha * 255.0).round() as u8);
        }
    }
    tray_icon::Icon::from_rgba(rgba, N, N).ok()
}

#[cfg(not(windows))]
pub struct Tray;

#[cfg(not(windows))]
impl Tray {
    pub fn new(_ctx: &egui::Context) -> Option<Self> {
        None
    }

    pub fn try_recv(&self) -> Option<TrayAction> {
        None
    }
}
