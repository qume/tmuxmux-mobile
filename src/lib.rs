//! Shared library + Android entry point.
//!
//! The desktop binary (`src/desktop.rs`) reuses `TmuxmuxApp` directly. On
//! Android, `android_main` is the entry the NativeActivity glue calls.

mod acs;
mod app;
mod colors;
pub mod config;
mod input;
mod render;
pub mod ssh;

pub use app::TmuxmuxApp;

/// Build the eframe options every platform shares.
pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        // We drive our own repaint pacing; vsync can stall while occluded.
        vsync: false,
        ..Default::default()
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    std::panic::set_hook(Box::new(|info| {
        log::error!("PANIC: {info}");
    }));

    let data_dir = app
        .internal_data_path()
        .unwrap_or_else(|| std::path::PathBuf::from("/data/local/tmp"));
    // A config dropped into the app's external files dir (adb push-able,
    // no extra permissions) is imported on launch.
    let import_dir = app.external_data_path();

    let mut options = native_options();
    // eframe 0.34 wires the winit event loop to the activity via this field.
    options.android_app = Some(app);

    log::info!("android_main: starting eframe");
    match eframe::run_native(
        "tmuxmux",
        options,
        Box::new(move |cc| Ok(Box::new(TmuxmuxApp::new(cc, data_dir, import_dir)))),
    ) {
        Ok(()) => log::info!("android_main: eframe exited cleanly"),
        Err(e) => log::error!("android_main: eframe::run_native failed: {e:?}"),
    }
}
