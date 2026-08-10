//! Desktop entry point — lets us build and test the whole app (UI + SSH)
//! on Linux/macOS/Windows without an Android device.

use std::path::PathBuf;

use tmuxmux_mobile::{native_options, TmuxmuxApp};

fn data_dir() -> PathBuf {
    // ~/.config/tmuxmux-mobile, falling back to the current dir.
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config").join("tmuxmux-mobile");
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("tmuxmux-mobile");
    }
    PathBuf::from(".")
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let dir = data_dir();
    let mut options = native_options();
    options.viewport = egui::ViewportBuilder::default()
        .with_inner_size([420.0, 780.0])
        .with_title("tmuxmux-mobile");

    let import = dir.clone();
    if let Err(e) = eframe::run_native(
        "tmuxmux-mobile",
        options,
        Box::new(move |cc| Ok(Box::new(TmuxmuxApp::new(cc, dir, Some(import))))),
    ) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
