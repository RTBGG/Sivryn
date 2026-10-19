//! Small per-user settings kept between starts, in
//! `%APPDATA%\Sivryn\settings.json` (or the platform's config dir).
//!
//! Only non-secret values live here; session tokens and keys belong in the
//! OS keychain once the client persists logins.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Instance URL of the last successful login.
    pub last_instance: Option<String>,
    /// Username of the last successful login.
    pub last_username: Option<String>,
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("Sivryn").join("settings.json"))
}

impl Settings {
    /// Loads settings; missing or unreadable files give the defaults.
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, serde_json::to_vec_pretty(self).expect("settings serialize")));
        if let Err(e) = result {
            tracing::warn!(error = %e, path = %path.display(), "could not save settings");
        }
    }
}
