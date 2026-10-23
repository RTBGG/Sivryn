//! Media quality settings shared between server (limits) and clients (encoder).

use serde::{Deserialize, Serialize};

/// Video codecs Sivryn negotiates, in order of preference for screen share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    Av1,
    H264,
    Vp9,
    Vp8,
}

/// Instance-wide limits for a screen share, set by the admin.
///
/// The defaults (1440p60 at 12 Mbit/s) are well above what Discord offers;
/// self-hosters can raise or lower them in the server configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenShareLimits {
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
    pub max_bitrate_bps: u64,
    /// Codec preference. The sender picks the first codec it can encode in
    /// hardware and that every subscriber can decode, falling back down the list.
    pub codec_preference: Vec<VideoCodec>,
}

impl Default for ScreenShareLimits {
    fn default() -> Self {
        Self {
            max_width: 2560,
            max_height: 1440,
            max_fps: 60,
            max_bitrate_bps: 12_000_000,
            codec_preference: vec![VideoCodec::Av1, VideoCodec::H264],
        }
    }
}

/// Limits for camera video, deliberately lower than screen share.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraLimits {
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
    pub max_bitrate_bps: u64,
}

impl Default for CameraLimits {
    fn default() -> Self {
        Self { max_width: 1920, max_height: 1080, max_fps: 60, max_bitrate_bps: 8_000_000 }
    }
}

/// What the screen share is optimized for. Maps to the encoder's
/// degradation preference: keep resolution (text) or keep frame rate (motion).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentHint {
    /// Code, documents, UIs: prefer sharpness, drop frames under pressure.
    #[default]
    Detail,
    /// Games and video: prefer smooth motion, drop resolution under pressure.
    Motion,
}
