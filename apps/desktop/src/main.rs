//! Sivryn desktop client (Dioxus Native: GPU-rendered UI without a webview).

// Release builds are GUI apps without a console window; debug builds keep the
// console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod chat;
mod diagnostics;
mod emoji;
mod files;
mod history;
mod hotkeys;
mod i18n;
mod links;
mod mentions;
mod messages;
mod native_icon;
mod notify;
mod preview;
mod session;
mod settings;
mod store;
mod taskbar;
mod tray;
mod ui;
mod updater;
mod video_surface;
mod window;

use dioxus_native::{LogicalSize, WindowAttributes};

fn main() {
    // First, so that even a start that dies early leaves a log behind.
    diagnostics::init();
    // Velopack's install and update hooks exit the process in here.
    updater::run_hooks();
    let settings = settings::Settings::load();
    i18n::set_language(settings.appearance.language);
    // Already running (perhaps hidden in the notification area): that one
    // shows its window (and gets the invite link this was started with), this
    // one ends.
    let link = links::from_args();
    if tray::already_running(link.as_deref()) {
        tracing::info!("handed over to the running Sivryn");
        return;
    }
    if let Some(invite) = link.as_deref().and_then(links::parse) {
        links::set_pending(invite);
    }
    links::register();
    // An update downloaded in an earlier session: install it and restart.
    updater::apply_pending();
    sivryn_client_core::install_crypto_provider();
    ui::register_fonts();

    // Started with Windows: only the notification area icon, if wanted.
    let hidden = autostart::launched_at_sign_in() && settings.start_minimized;
    let window = WindowAttributes::default()
        .with_visible(!hidden)
        .with_title("Sivryn")
        .with_window_icon(window::icon())
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(900.0, 560.0));

    // dioxus-native runs a tokio runtime on the UI thread, so network futures
    // can be awaited directly from components.
    tracing::info!(hidden, "opening the window");
    dioxus_native::launch_cfg(ui::App, Vec::new(), vec![Box::new(window)]);
    tracing::info!("window closed");
}
