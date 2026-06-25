//! Terminull — launch, minimise and spawn terminals like lightweight VMs.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod grid;
mod terminal;

use app::TerminullApp;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([640.0, 420.0])
            .with_title("Terminull"),
        ..Default::default()
    };

    eframe::run_native(
        "Terminull",
        options,
        Box::new(|_cc| Ok(Box::<TerminullApp>::default())),
    )
}
