//! Sivryn desktop client (Dioxus Native: GPU-rendered UI without a webview).

mod chat;
mod session;
mod ui;
mod video_surface;

use dioxus_native::{LogicalSize, WindowAttributes};
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,sivryn_desktop=debug,sivryn_media=debug,sivryn_client_core=debug".into()),
        )
        .init();

    let window = WindowAttributes::default()
        .with_title("Sivryn")
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(900.0, 560.0));

    // dioxus-native runs a tokio runtime on the UI thread, so network futures
    // can be awaited directly from components.
    dioxus_native::launch_cfg(ui::App, Vec::new(), vec![Box::new(window)]);
}
