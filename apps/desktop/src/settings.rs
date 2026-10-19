//! Per-user settings kept between starts, in `%APPDATA%\Sivryn\settings.json`
//! (or the platform's config dir).
//!
//! Only non-secret values live here; session tokens and the key for the local
//! message history are in the OS credential store (see `store`).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sivryn_client_core::protocol::GuildId;
use sivryn_client_core::protocol::media::{ContentHint, ScreenShareLimits};
use sivryn_media::{AudioConfig, CameraQuality};

use crate::hotkeys::Keybind;
use crate::store::AccountId;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Instance URL of the last successful login.
    pub last_instance: Option<String>,
    /// Username of the last successful login.
    pub last_username: Option<String>,
    /// Account that signs in automatically (its token is in the credential store).
    pub auto_login: Option<AccountId>,
    pub last_guild: Option<GuildId>,
    pub audio: AudioConfig,
    /// Per-participant volume (identity, or `identity:screen` for share audio).
    pub volumes: HashMap<String, f32>,
    pub video: VideoSettings,
    pub keybinds: Keybinds,
    pub appearance: Appearance,
    pub notifications: Notifications,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoSettings {
    /// Camera name; `None` uses the first camera.
    pub camera: Option<String>,
    pub camera_quality: CameraQuality,
    pub screen_quality: ScreenQuality,
    pub screen_hint: ContentHint,
    /// Share the shared window's (or the system's) sound along with the picture.
    pub screen_audio: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            camera: None,
            camera_quality: CameraQuality::default(),
            screen_quality: ScreenQuality::default(),
            screen_hint: ContentHint::Detail,
            screen_audio: true,
        }
    }
}

/// Screen share resolution and frame rate; the server's limits still apply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenQuality {
    P1080_30,
    P1080_60,
    #[default]
    P1440_60,
    P2160_30,
}

impl ScreenQuality {
    pub const ALL: [ScreenQuality; 4] =
        [ScreenQuality::P1080_30, ScreenQuality::P1080_60, ScreenQuality::P1440_60, ScreenQuality::P2160_30];

    pub fn label(self) -> &'static str {
        match self {
            ScreenQuality::P1080_30 => "1080p · 30 fps",
            ScreenQuality::P1080_60 => "1080p · 60 fps",
            ScreenQuality::P1440_60 => "1440p · 60 fps",
            ScreenQuality::P2160_30 => "4K · 30 fps",
        }
    }

    /// The server's limits narrowed to this preference.
    pub fn apply(self, server: &ScreenShareLimits) -> ScreenShareLimits {
        let (w, h, fps) = match self {
            ScreenQuality::P1080_30 => (1920, 1080, 30),
            ScreenQuality::P1080_60 => (1920, 1080, 60),
            ScreenQuality::P1440_60 => (2560, 1440, 60),
            ScreenQuality::P2160_30 => (3840, 2160, 30),
        };
        ScreenShareLimits {
            max_width: server.max_width.min(w),
            max_height: server.max_height.min(h),
            max_fps: server.max_fps.min(fps),
            ..server.clone()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keybinds {
    pub push_to_talk: Option<Keybind>,
    pub toggle_mute: Option<Keybind>,
    pub toggle_deafen: Option<Keybind>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Dark,
    Light,
    #[default]
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    System,
    German,
    English,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
    pub language: Language,
    /// Base font size in px (13–18).
    pub font_size: u8,
    /// Tighter message list without avatars on every line.
    pub compact: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { theme: Theme::default(), language: Language::default(), font_size: 15, compact: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    /// Sound for messages in channels you're not looking at.
    pub message_sound: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self { message_sound: true }
    }
}

/// `%APPDATA%\Sivryn` (or `$XDG_CONFIG_HOME/Sivryn`, `~/.config/Sivryn`).
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("Sivryn"))
}

fn path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.json"))
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
        // Write to a temporary file first so a crash never leaves half a file.
        let tmp = path.with_extension("json.tmp");
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&tmp, serde_json::to_vec_pretty(self).expect("settings serialize")))
            .and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(e) = result {
            tracing::warn!(error = %e, path = %path.display(), "could not save settings");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_files_still_load() {
        let old = r#"{ "last_instance": "https://chat.example.com", "last_username": "rtb" }"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(s.last_username.as_deref(), Some("rtb"));
        assert_eq!(s.audio, AudioConfig::default());
        assert_eq!(s.appearance.font_size, 15);
        assert!(s.video.screen_audio);
    }

    #[test]
    fn screen_quality_respects_server_limits() {
        let server = ScreenShareLimits::default(); // 1440p60
        let four_k = ScreenQuality::P2160_30.apply(&server);
        assert_eq!((four_k.max_width, four_k.max_height, four_k.max_fps), (2560, 1440, 30));
        let low = ScreenQuality::P1080_30.apply(&server);
        assert_eq!((low.max_width, low.max_height, low.max_fps), (1920, 1080, 30));
        assert_eq!(low.max_bitrate_bps, server.max_bitrate_bps);
    }
}
