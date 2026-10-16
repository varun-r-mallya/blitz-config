#![cfg_attr(windows, windows_subsystem = "windows")]

use blitz_config::app::App;

const USAGE: &str = "blitz-config [--dry-run]

  --dry-run   connect and read normally, but log state-changing reports
              (writes, rumble, calibration, profile switches) instead of sending them";

fn main() -> eframe::Result {
    let mut dry_run = false;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--version" | "-V" => {
                println!("blitz-config {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => {
                println!("{USAGE}");
                return Ok(());
            }
        }
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Blitz Configurator")
            .with_app_id("blitz-config")
            .with_inner_size([1220.0, 780.0])
            .with_min_inner_size([980.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native("Blitz Configurator", options, Box::new(move |_cc| Ok(Box::new(App::new(dry_run)))))
}
