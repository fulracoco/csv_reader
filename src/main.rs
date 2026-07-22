#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod csv_engine;

fn main() -> eframe::Result {
    app::run()
}
