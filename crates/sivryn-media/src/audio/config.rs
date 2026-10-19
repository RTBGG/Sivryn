//! User-facing audio settings, persisted by the client.

use serde::{Deserialize, Serialize};

/// Which noise filter runs on the microphone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoiseSuppression {
    /// DeepFilterNet3: best quality (keyboards, fans, background voices),
    /// ~1–3 % of one CPU core, ~30 ms extra latency.
    #[default]
    DeepFilter,
    /// RNNoise: lightweight neural filter, 10 ms extra latency.
    RnNoise,
    /// WebRTC's classic spectral noise suppressor.
    WebRtc,
    Off,
}

/// When the microphone transmits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceMode {
    #[default]
    VoiceActivity,
    PushToTalk,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// cpal device ID; `None` follows the system default.
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    /// Linear gains, 0.0–2.0.
    pub input_volume: f32,
    pub output_volume: f32,
    pub noise_suppression: NoiseSuppression,
    pub echo_cancellation: bool,
    pub auto_gain: bool,
    pub voice_mode: VoiceMode,
    /// Voice activity: let the speech detector decide instead of a fixed level.
    pub auto_sensitivity: bool,
    /// Voice activity: transmit above this level (dBFS, -80–0).
    pub sensitivity_db: f32,
    /// Push-to-talk: keep transmitting this long after the key is released.
    pub ptt_release_ms: u32,
    /// Join/leave/mute sounds.
    pub sounds: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            input_volume: 1.0,
            output_volume: 1.0,
            noise_suppression: NoiseSuppression::default(),
            echo_cancellation: true,
            auto_gain: true,
            voice_mode: VoiceMode::default(),
            auto_sensitivity: true,
            sensitivity_db: -50.0,
            ptt_release_ms: 200,
            sounds: true,
        }
    }
}
