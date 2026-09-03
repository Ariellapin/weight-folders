#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod app;
mod model;
mod scanner;
mod snapshot;
mod ui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("Weight Folders"),
        ..Default::default()
    };
    eframe::run_native(
        "Weight Folders",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
