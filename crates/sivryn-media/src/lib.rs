//! Sivryn's native media engine.
//!
//! - [`screen`]: captures a screen or window (Windows Graphics Capture,
//!   ScreenCaptureKit, PipeWire, all via libwebrtc's desktop capturer) and
//!   publishes it with the best hardware codec available.
//! - [`call`]: joins a voice channel's LiveKit room, publishes the microphone
//!   and hands remote video to the UI as RGBA frames.
//! - [`audio`]: Sivryn's audio engine (noise filters, echo cancellation,
//!   voice activity / push-to-talk, mixing with per-user volume).
//! - [`codec`]: hardware encoder detection and codec selection.

pub mod audio;
pub mod call;
pub mod camera;
pub mod codec;
mod cursor;
mod hdr_capture;
pub mod player;
pub mod screen;
pub mod screen_audio;
pub mod thumbnails;
pub mod video;

pub use audio::{AudioConfig, AudioDevice, AudioEngine, NoiseSuppression, Sound, VoiceMode};
pub use call::{CallEvent, DEAFENED_ATTRIBUTE, VoiceCall, screen_volume_key};
pub use camera::{CameraDevice, CameraQuality, CameraShare, list_cameras};
pub use livekit::prelude::TrackSource;
pub use player::{MediaPlayer, video_poster};
pub use screen::{ScreenShare, ScreenSource, SourceKind, list_sources};
pub use screen_audio::{LoopbackTarget, ScreenAudio, window_process};
pub use thumbnails::{SourcePreviews, Thumbnail};
pub use video::{RgbaFrame, VideoFeed};

/// Whether Windows Media Foundation is installed. "N" editions of Windows
/// ship without it unless the Media Feature Pack is added. The desktop app
/// delay-loads its DLLs, so this must be checked before any Media Foundation
/// call (hardware encoder detection, cameras); without it the app still runs
/// with software encoding and no camera.
pub fn media_foundation_available() -> bool {
    #[cfg(windows)]
    {
        use std::sync::OnceLock;
        static AVAILABLE: OnceLock<bool> = OnceLock::new();
        *AVAILABLE.get_or_init(|| {
            use windows::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
            // SAFETY: loads a system DLL by name; the handle stays loaded on purpose.
            let ok =
                unsafe { LoadLibraryExW(windows::core::w!("mfplat.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.is_ok();
            if !ok {
                tracing::warn!(
                    "Media Foundation is missing (Windows N without Media Feature Pack): no hardware encoding or camera"
                );
            }
            ok
        })
    }
    #[cfg(not(windows))]
    {
        true
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("LiveKit: {0}")]
    LiveKit(#[from] livekit::RoomError),
    #[error("audio device: {0}")]
    Audio(String),
    #[error("screen capture unavailable on this system")]
    CaptureUnavailable,
    #[error("capture source {0} not found")]
    SourceNotFound(u64),
    #[error("no camera found")]
    NoCamera,
    #[error("cameras need Windows Media Foundation (install the Media Feature Pack on Windows N)")]
    NoMediaFoundation,
    #[error("camera: {0}")]
    Camera(String),
}
