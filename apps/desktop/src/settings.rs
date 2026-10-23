//! Per-user settings kept between starts, in `%APPDATA%\Sivryn\settings.json`
//! (or the platform's config dir).
//!
//! Only non-secret values live here; session tokens and the key for the local
//! message history are in the OS credential store (see `store`).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sivryn_client_core::protocol::media::{ContentHint, ScreenShareLimits};
use sivryn_client_core::protocol::{CategoryId, ChannelId, GuildId};
use sivryn_media::{AudioConfig, CameraQuality};

use crate::hotkeys::Keybind;
use crate::store::AccountId;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Instance URL of the last successful login.
    pub last_instance: Option<String>,
    /// Username of the last successful login.
    pub last_username: Option<String>,
    /// Account that signs in automatically (its token is in the credential store).
    pub auto_login: Option<AccountId>,
    pub last_guild: Option<GuildId>,
    /// The direct message opened last; the Sivryn logo goes back to it.
    pub last_dm: Option<ChannelId>,
    pub audio: AudioConfig,
    /// Per-participant volume (identity, or `identity:screen` for share audio).
    pub volumes: HashMap<String, f32>,
    /// Participants muted for this user only.
    pub muted_users: std::collections::HashSet<String>,
    /// Member list on the right of guild channels.
    pub member_list: bool,
    /// Attach previews of the first link to messages you send.
    pub link_previews: bool,
    /// Folded channel categories.
    pub collapsed_categories: std::collections::HashSet<CategoryId>,
    /// Ask before leaving one voice channel for another.
    pub confirm_voice_switch: bool,
    /// Download updates by themselves (installed at the next start).
    pub auto_update: bool,
    /// Widths of the channel list and of a voice channel's chat, in pixels.
    pub sidebar_width: f64,
    pub voice_chat_width: f64,
    pub video: VideoSettings,
    pub keybinds: Keybinds,
    pub appearance: Appearance,
    pub notifications: Notifications,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            last_instance: None,
            last_username: None,
            auto_login: None,
            last_guild: None,
            last_dm: None,
            audio: AudioConfig::default(),
            volumes: HashMap::new(),
            muted_users: Default::default(),
            member_list: true,
            link_previews: true,
            collapsed_categories: Default::default(),
            confirm_voice_switch: true,
            auto_update: true,
            sidebar_width: 252.0,
            voice_chat_width: 380.0,
            video: VideoSettings::default(),
            keybinds: Keybinds::default(),
            appearance: Appearance::default(),
            notifications: Notifications::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoSettings {
    /// Camera name; `None` uses the first camera.
    pub camera: Option<String>,
    pub camera_quality: CameraQuality,
    /// Screen share resolution and frame rate (the last ones used).
    pub screen_resolution: ScreenResolution,
    pub screen_fps: u32,
    pub screen_hint: ContentHint,
    /// Share the shared window's (or the system's) sound along with the picture.
    pub screen_audio: bool,
    /// The share picker's last tab: windows (`true`) or screens.
    pub share_windows_tab: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            camera: None,
            camera_quality: CameraQuality::default(),
            screen_resolution: ScreenResolution::default(),
            screen_fps: 60,
            screen_hint: ContentHint::Detail,
            screen_audio: true,
            share_windows_tab: false,
        }
    }
}

/// Screen share resolution; the server's limits still apply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenResolution {
    P720,
    #[default]
    P1080,
    P1440,
    P2160,
}

/// Frame rates offered for screen sharing.
pub const SCREEN_FPS: [u32; 3] = [15, 30, 60];

impl ScreenResolution {
    pub const ALL: [ScreenResolution; 4] =
        [ScreenResolution::P720, ScreenResolution::P1080, ScreenResolution::P1440, ScreenResolution::P2160];

    pub fn label(self) -> &'static str {
        match self {
            ScreenResolution::P720 => "720p",
            ScreenResolution::P1080 => "1080p",
            ScreenResolution::P1440 => "1440p",
            ScreenResolution::P2160 => "4K",
        }
    }

    fn size(self) -> (u32, u32) {
        match self {
            ScreenResolution::P720 => (1280, 720),
            ScreenResolution::P1080 => (1920, 1080),
            ScreenResolution::P1440 => (2560, 1440),
            ScreenResolution::P2160 => (3840, 2160),
        }
    }
}

impl VideoSettings {
    /// The server's limits narrowed to the chosen resolution and frame rate.
    pub fn screen_limits(&self, server: &ScreenShareLimits) -> ScreenShareLimits {
        let (w, h) = self.screen_resolution.size();
        ScreenShareLimits {
            max_width: server.max_width.min(w),
            max_height: server.max_height.min(h),
            max_fps: server.max_fps.min(self.screen_fps.max(1)),
            ..server.clone()
        }
    }

    pub fn screen_label(&self) -> String {
        format!("{} · {} fps", self.screen_resolution.label(), self.screen_fps)
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
    /// Windows notifications for new messages.
    pub desktop: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self { message_sound: true, desktop: true }
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
        let mut video =
            VideoSettings { screen_resolution: ScreenResolution::P2160, screen_fps: 30, ..Default::default() };
        let four_k = video.screen_limits(&server);
        assert_eq!((four_k.max_width, four_k.max_height, four_k.max_fps), (2560, 1440, 30));
        video.screen_resolution = ScreenResolution::P720;
        video.screen_fps = 15;
        let low = video.screen_limits(&server);
        assert_eq!((low.max_width, low.max_height, low.max_fps), (1280, 720, 15));
        assert_eq!(low.max_bitrate_bps, server.max_bitrate_bps);
        assert_eq!(video.screen_label(), "720p · 15 fps");
    }
}
