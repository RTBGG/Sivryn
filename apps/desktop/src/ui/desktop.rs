//! Sivryn on the Windows desktop: the notification area icon, the call
//! buttons in the taskbar preview, closing to the notification area and
//! starting with Windows. The pieces live in `tray.rs`, `taskbar.rs` and
//! `autostart.rs`; this connects them to the app's state.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::model::UserStatus;

use super::AppState;
use crate::i18n::t;
use crate::tray::{Badge, ShellEvent};
use crate::{taskbar, tray};

/// The close button hides the window to the notification area (setting).
static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(true);

/// Starts the integration (once, from the app's root).
pub fn start(state: AppState) {
    blitz_shell::window_control::set_close_handler(|| CLOSE_TO_TRAY.load(Ordering::Relaxed));
    let mut events = tray::start(tray::Labels {
        open: t("tray.open").into(),
        check_updates: t("update.check").into(),
        quit: t("tray.quit").into(),
    });
    spawn_forever(async move {
        while let Some(event) = events.recv().await {
            handle(state, event);
        }
    });

    // Keep the tray icon and the taskbar buttons in step with the app.
    let labels = taskbar::Labels {
        camera_on: t("call.camera_on").into(),
        camera_off: t("call.camera_off").into(),
        mute: t("call.mute").into(),
        unmute: t("call.unmute").into(),
        deafen: t("call.deafen").into(),
        undeafen: t("call.undeafen").into(),
        hang_up: t("call.leave").into(),
    };
    spawn_forever(async move {
        // The window must exist before its taskbar button can get buttons.
        tokio::time::sleep(Duration::from_millis(500)).await;
        loop {
            tray::set_badge(badge(state));
            let call = state.call.peek().as_ref().map(|c| taskbar::CallButtons {
                camera: c.camera_on,
                muted: *state.muted.peek() || state.in_afk_channel(),
                deafened: *state.deafened.peek(),
            });
            taskbar::update(call, &labels);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}

/// What the tray icon shows: a voice bubble in a call (filled while I
/// speak), my status otherwise.
fn badge(state: AppState) -> Badge {
    if state.call.peek().is_some() {
        let speaking = super::use_services_engine().transmitting();
        return Badge::Voice { speaking };
    }
    match *state.my_status.peek() {
        UserStatus::Online => Badge::Online,
        UserStatus::Idle => Badge::Idle,
        UserStatus::Dnd => Badge::Busy,
        UserStatus::Invisible => Badge::Offline,
    }
}

fn handle(mut state: AppState, event: ShellEvent) {
    tracing::debug!(?event, "shell event");
    match event {
        ShellEvent::Open => blitz_shell::window_control::show_and_focus(),
        ShellEvent::Invite => {
            blitz_shell::window_control::show_and_focus();
            super::invites::arrived(state);
        }
        ShellEvent::CheckForUpdates => {
            blitz_shell::window_control::show_and_focus();
            state.settings_page.set(Some(super::SettingsPage::About));
            spawn_forever(super::updates::check(state));
        }
        ShellEvent::Quit => quit(state),
        // Read first: a read guard still alive while setting the signal crashed.
        ShellEvent::ToggleMute => {
            let muted = *state.muted.peek();
            state.set_muted(!muted);
        }
        ShellEvent::ToggleDeafen => {
            let deafened = *state.deafened.peek();
            state.set_deafened(!deafened);
        }
        ShellEvent::ToggleCamera => super::call::toggle_camera(state),
        ShellEvent::HangUp => super::call::hang_up(state),
    }
}

/// Quits for real (the close button may only hide the window).
pub fn quit(state: AppState) {
    if state.call.peek().is_some() {
        super::call::hang_up(state);
    }
    tray::remove();
    spawn_forever(async move {
        // A moment for leaving the call to reach the server.
        tokio::time::sleep(Duration::from_millis(300)).await;
        std::process::exit(0);
    });
}

/// Applies the Windows settings (called whenever settings change).
pub fn apply_settings(close_to_tray: bool, autostart: bool) {
    CLOSE_TO_TRAY.store(close_to_tray, Ordering::Relaxed);
    static APPLIED: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);
    let mut applied = APPLIED.lock().unwrap_or_else(|e| e.into_inner());
    if *applied != Some(autostart) {
        *applied = Some(autostart);
        crate::autostart::apply(autostart);
    }
}
