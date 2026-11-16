// Pas de console noire en version release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod edit;
mod highlight;
mod local;
mod mobaxterm;
mod monitor;
mod session;
mod ssh;
mod terminal;
mod theme;
mod vault;
mod win_frame;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Ferroterm")
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: include_bytes!("../assets/ferroterm-64.rgba").to_vec(),
                width: 64,
                height: 64,
            }))
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Ferroterm",
        options,
        Box::new(|cc| Ok(Box::new(app::FerroApp::new(cc)))),
    )
}
