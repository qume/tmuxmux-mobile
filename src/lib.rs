//! Shared library + Android entry point.
//!
//! The desktop binary (`src/desktop.rs`) reuses `TmuxmuxApp` directly. On
//! Android, `android_main` is the entry the NativeActivity glue calls.

mod acs;
mod app;
mod colors;
mod config;
mod input;
mod render;
mod ssh;

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
    use winit::platform::android::EventLoopBuilderExtAndroid;

    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    let data_dir = app
        .internal_data_path()
        .unwrap_or_else(|| std::path::PathBuf::from("/data/local/tmp"));

    let app_for_loop = app.clone();
    let mut options = native_options();
    options.event_loop_builder = Some(Box::new(move |builder| {
        builder.with_android_app(app_for_loop);
    }));

    let _ = eframe::run_native(
        "tmuxmux",
        options,
        Box::new(move |cc| Ok(Box::new(TmuxmuxApp::new(cc, data_dir)))),
    );
}
