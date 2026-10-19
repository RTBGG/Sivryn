//! Sivryn's native media engine.
//!
//! - [`screen`]: captures a screen or window (Windows Graphics Capture,
//!   ScreenCaptureKit, PipeWire, all via libwebrtc's desktop capturer) and
//!   publishes it with the best hardware codec available.
//! - [`call`]: joins a voice channel's LiveKit room, publishes the microphone
//!   and hands remote video to the UI as RGBA frames.
//! - [`codec`]: hardware encoder detection and codec selection.

pub mod call;
pub mod codec;
pub mod screen;
pub mod video;

pub use call::{CallEvent, VoiceCall};
pub use livekit::prelude::TrackSource;
pub use screen::{ScreenShare, ScreenSource, SourceKind, list_sources};
pub use video::{RgbaFrame, VideoFeed};

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
}
