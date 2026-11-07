// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod media;
mod tray;
mod ui;

use eframe::egui;

fn main() -> eframe::Result<()> {
    // `dpmedia --reset-position` ignores the saved position (e.g. after
    // unplugging the monitor the widget used to sit on). Pins are kept.
    let reset = std::env::args().any(|a| a == "--reset-position");
    let mut cfg = config::Config::load().unwrap_or_default();
    if reset {
        cfg.x = config::DEFAULT_X;
        cfg.y = config::DEFAULT_Y;
        cfg.phys_x = None;
        cfg.phys_y = None;
    }

    let viewport = egui::ViewportBuilder::default()
        .with_title("dpMedia")
        // One fixed size. min = max also makes Windows clamp the window back to
        // exactly this (logical) size when it crosses to a monitor with a
        // different scale factor.
        .with_inner_size([ui::WINDOW_W, ui::PILL_H])
        .with_min_inner_size([ui::WINDOW_W, ui::PILL_H])
        .with_max_inner_size([ui::WINDOW_W, ui::PILL_H])
        .with_position(egui::pos2(cfg.x, cfg.y))
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_resizable(false)
        // No button on the main taskbar; the tray icon is the handle.
        .with_taskbar(false);

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "dpMedia",
        options,
        Box::new(move |cc| {
            // The tray handlers and the worker thread get Context clones so
            // they can wake the UI; nothing ever polls.
            let tray = tray::Tray::new(&cc.egui_ctx);
            let media = media::spawn(cc.egui_ctx.clone(), cfg.filter());
            Ok(Box::new(ui::DpMedia::new(&cc.egui_ctx, media, tray, cfg)))
        }),
    )
}
