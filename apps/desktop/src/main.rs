//! Sivryn desktop client (Dioxus Native: GPU-rendered UI without a webview).

// Release builds are GUI apps without a console window; debug builds keep the
// console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chat;
mod emoji;
mod files;
mod hotkeys;
mod i18n;
mod mentions;
mod messages;
mod notify;
mod preview;
mod session;
mod settings;
mod store;
mod ui;
mod video_surface;
mod window;

use dioxus_native::{LogicalSize, WindowAttributes};
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,sivryn_desktop=info,sivryn_media=info,sivryn_client_core=info".into()),
        )
        .init();
    install_crash_log();
    sivryn_client_core::install_crypto_provider();
    ui::register_fonts();

    let window = WindowAttributes::default()
        .with_title("Sivryn")
        .with_window_icon(window::icon())
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(900.0, 560.0));

    // dioxus-native runs a tokio runtime on the UI thread, so network futures
    // can be awaited directly from components.
    dioxus_native::launch_cfg(ui::App, Vec::new(), vec![Box::new(window)]);
}

/// Release builds have no console, so panics would vanish without a trace.
/// Append them to `%LOCALAPPDATA%\Sivryn\crash.log` (or the temp dir).
fn install_crash_log() {
    let dir = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let path = dir.join("Sivryn").join("crash.log");
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let report = format!(
            "--- sivryn-desktop {} crashed (unix time {secs})\n{info}\n{}\n",
            env!("CARGO_PKG_VERSION"),
            std::backtrace::Backtrace::force_capture(),
        );
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = file.write_all(report.as_bytes());
        }
        default_hook(info);
    }));
}
