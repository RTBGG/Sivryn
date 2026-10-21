//! Updates in the app: checked at start and every few hours, downloaded in
//! the background, then a bar offers to restart into the new version (it is
//! installed at the next start otherwise). The About page shows the state and
//! checks on demand.

use std::time::Duration;

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;

use super::AppState;
use super::icons::{Icon, Svg};
use crate::i18n::t;
use crate::updater::{self, Found};

/// Time between automatic checks.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    Downloading(u8),
    UpToDate,
    /// Downloaded; installed on restart.
    Ready(String),
    Failed(String),
    /// Run from a build folder, not installed: can't update itself.
    NotInstalled,
}

/// Checks automatically, shortly after start and then every few hours.
pub fn start(state: AppState) {
    spawn_forever(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        loop {
            if state.settings.peek().auto_update {
                check(state).await;
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// Looks for an update and downloads it.
pub async fn check(mut state: AppState) {
    if !updater::is_installed() {
        state.update.set(UpdateState::NotInstalled);
        return;
    }
    if matches!(*state.update.peek(), UpdateState::Checking | UpdateState::Downloading(_) | UpdateState::Ready(_)) {
        return;
    }
    state.update.set(UpdateState::Checking);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::task::spawn_blocking(move || {
        updater::check_and_download(move |p| {
            let _ = tx.send(p);
        })
    });
    // The channel closes when the download is over.
    while let Some(p) = rx.recv().await {
        state.update.set(UpdateState::Downloading(p));
    }
    let next = match task.await.map_err(|e| e.to_string()).and_then(|r| r) {
        Ok(Found::UpToDate) => UpdateState::UpToDate,
        Ok(Found::Ready(version)) => UpdateState::Ready(version),
        Err(e) => {
            tracing::warn!(error = %e, "update check failed");
            UpdateState::Failed(e)
        }
    };
    state.update.set(next);
}

fn restart(mut state: AppState) {
    if let Err(e) = updater::restart_now() {
        state.update.set(UpdateState::Failed(e));
    }
}

/// "Version x is ready", over the app while an update waits for a restart.
#[component]
pub fn UpdateBar() -> Element {
    let state = use_context::<AppState>();
    let mut dismissed = use_signal(|| false);
    let UpdateState::Ready(version) = state.update.read().clone() else { return rsx! {} };
    if *dismissed.read() {
        return rsx! {};
    }
    rsx! {
        div { class: "update-bar",
            Svg { icon: Icon::Download, size: 16 }
            span { "Sivryn {version} {t(\"update.ready\")}" }
            button { class: "button small", onclick: move |_| restart(state), {t("update.restart")} }
            span { class: "link", onclick: move |_| dismissed.set(true), {t("update.later")} }
        }
    }
}

/// The update part of the About page.
#[component]
pub fn UpdateSection() -> Element {
    let state = use_context::<AppState>();
    let current = use_hook(updater::current_version);
    let status = state.update.read().clone();
    let auto = state.settings.read().auto_update;
    let busy = matches!(status, UpdateState::Checking | UpdateState::Downloading(_));
    let text = match &status {
        UpdateState::Idle => String::new(),
        UpdateState::Checking => t("update.checking").into(),
        UpdateState::Downloading(p) => format!("{} {p} %", t("update.downloading")),
        UpdateState::UpToDate => t("update.up_to_date").into(),
        UpdateState::Ready(v) => format!("Sivryn {v} {}", t("update.ready")),
        UpdateState::Failed(e) => format!("{}: {e}", t("update.failed")),
        UpdateState::NotInstalled => t("update.not_installed").into(),
    };
    rsx! {
        h2 { class: "subsection", {t("update.title")} }
        div { class: "update-row",
            div {
                div { "{t(\"update.current\")} {current}" }
                if !text.is_empty() {
                    div { class: "muted small", "{text}" }
                }
            }
            match status {
                UpdateState::Ready(_) => rsx! {
                    button { class: "button primary", onclick: move |_| restart(state), {t("update.restart")} }
                },
                _ if busy => rsx! { div { class: "spinner small" } },
                _ => rsx! {
                    button { class: "button", onclick: move |_| { spawn_forever(check(state)); }, {t("update.check")} }
                },
            }
        }
        super::settings_view::ToggleRow {
            label: t("update.auto").to_string(),
            hint: Some(t("update.auto_hint").to_string()),
            on: auto,
            onchange: move |v| state.update_settings(|s| s.auto_update = v),
        }
    }
}
