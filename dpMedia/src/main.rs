// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod media;
mod ui;

use eframe::egui;

const DEFAULT_POS: egui::Pos2 = egui::pos2(40.0, 40.0);

fn main() -> eframe::Result<()> {
    // `dpmedia --reset-position` ignores the saved position (e.g. after
    // unplugging the monitor the widget used to sit on).
    let reset = std::env::args().any(|a| a == "--reset-position");
    let pos = if reset {
        DEFAULT_POS
    } else {
        config::Config::load()
            .map(|c| egui::pos2(c.x, c.y))
            .unwrap_or(DEFAULT_POS)
    };

    let viewport = egui::ViewportBuilder::default()
        .with_title("dpMedia")
        .with_inner_size([ui::WINDOW_W, ui::WINDOW_H])
        .with_position(pos)
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_resizable(false);

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "dpMedia",
        options,
        Box::new(|cc| {
            // The worker thread gets a Context clone so it can wake the UI.
            let media = media::spawn(cc.egui_ctx.clone());
            Ok(Box::new(ui::DpMedia::new(media)))
        }),
    )
}
