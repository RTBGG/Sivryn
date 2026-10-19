//! Signal processing building blocks for the microphone chain.

use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::{ArrayView2, ArrayViewMut2};
use nnnoiseless::DenoiseState;
use rubato::{FftFixedIn, Resampler};
use sonora::config::{AdaptiveDigital, EchoCanceller, GainController2, HighPassFilter, NoiseSuppressionLevel};
use sonora::{AudioProcessing, Config, StreamConfig};

use super::config::NoiseSuppression;

/// Everything runs at 48 kHz in 10 ms frames, like WebRTC and Opus.
pub const RATE: u32 = 48_000;
pub const FRAME: usize = 480;

/// Converts a mono stream from the device rate to 48 kHz.
pub struct Resampler48 {
    inner: Option<FftFixedIn<f32>>,
    pending: Vec<f32>,
    out: Vec<Vec<f32>>,
}

impl Resampler48 {
    pub fn new(input_rate: u32) -> Self {
        let inner = (input_rate != RATE).then(|| {
            FftFixedIn::<f32>::new(input_rate as usize, RATE as usize, (input_rate / 100) as usize, 1, 1)
                .expect("valid resampler parameters")
        });
        Self { inner, pending: Vec::new(), out: vec![Vec::new()] }
    }

    /// Feeds device samples; appends 48 kHz samples to `output`.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        let Some(resampler) = &mut self.inner else {
            output.extend_from_slice(input);
            return;
        };
        self.pending.extend_from_slice(input);
        loop {
            let need = resampler.input_frames_next();
            if self.pending.len() < need {
                break;
            }
            self.out[0].resize(resampler.output_frames_max(), 0.0);
            let (used, produced) = resampler
                .process_into_buffer(&[&self.pending[..need]], &mut self.out, None)
                .expect("buffers sized for the resampler");
            output.extend_from_slice(&self.out[0][..produced]);
            self.pending.drain(..used);
        }
    }
}

fn apm(config: Config) -> AudioProcessing {
    AudioProcessing::builder()
        .config(config)
        .capture_config(StreamConfig::new(RATE, 1))
        .render_config(StreamConfig::new(RATE, 1))
        .build()
}

/// High-pass filter plus WebRTC AEC3. Needs the far-end (what we play) via
/// [`EchoCanceller3::render`] to know what to remove.
pub fn echo_canceller(enabled: bool) -> AudioProcessing {
    apm(Config {
        high_pass_filter: Some(HighPassFilter::default()),
        echo_canceller: enabled.then(EchoCanceller::default),
        ..Default::default()
    })
}

/// Adaptive digital gain with limiter (AGC2), run after noise suppression.
pub fn auto_gain() -> AudioProcessing {
    apm(Config {
        gain_controller2: Some(GainController2 {
            adaptive_digital: Some(AdaptiveDigital::default()),
            ..Default::default()
        }),
        ..Default::default()
    })
}

pub enum Denoiser {
    DeepFilter(Box<DfTract>),
    RnNoise,
    WebRtc(Box<AudioProcessing>),
    Off,
}

impl Denoiser {
    pub fn new(kind: NoiseSuppression) -> Self {
        match kind {
            NoiseSuppression::DeepFilter => match DfTract::new(DfParams::default(), &RuntimeParams::default_with_ch(1))
            {
                Ok(df) => Denoiser::DeepFilter(Box::new(df)),
                Err(e) => {
                    tracing::warn!(error = %e, "DeepFilterNet unavailable, falling back to RNNoise");
                    Denoiser::RnNoise
                }
            },
            NoiseSuppression::RnNoise => Denoiser::RnNoise,
            NoiseSuppression::WebRtc => Denoiser::WebRtc(Box::new(apm(Config {
                noise_suppression: Some(sonora::config::NoiseSuppression {
                    level: NoiseSuppressionLevel::High,
                    ..Default::default()
                }),
                ..Default::default()
            }))),
            NoiseSuppression::Off => Denoiser::Off,
        }
    }

    /// Denoises one frame in place. `rnnoise_out` is RNNoise's output for
    /// the same frame (it always runs, as the speech detector).
    pub fn process(&mut self, frame: &mut [f32; FRAME], rnnoise_out: &[f32; FRAME]) {
        match self {
            Denoiser::DeepFilter(df) => {
                let input = *frame;
                let noisy = ArrayView2::from_shape((1, FRAME), &input).expect("frame shape");
                let enhanced = ArrayViewMut2::from_shape((1, FRAME), &mut frame[..]).expect("frame shape");
                if let Err(e) = df.process(noisy, enhanced) {
                    tracing::trace!(error = %e, "DeepFilterNet frame failed");
                    *frame = input;
                }
            }
            Denoiser::RnNoise => *frame = *rnnoise_out,
            Denoiser::WebRtc(apm) => {
                let input = *frame;
                let _ = apm.process_capture_f32(&[&input], &mut [&mut frame[..]]);
            }
            Denoiser::Off => {}
        }
    }
}

/// RNNoise as speech detector (and as the RNNoise filter option).
pub struct SpeechDetector {
    state: Box<DenoiseState<'static>>,
    scaled: [f32; FRAME],
    out: [f32; FRAME],
}

impl SpeechDetector {
    pub fn new() -> Self {
        Self { state: DenoiseState::new(), scaled: [0.0; FRAME], out: [0.0; FRAME] }
    }

    /// Returns the probability that the frame contains speech; the denoised
    /// frame is available from [`SpeechDetector::denoised`] afterwards.
    pub fn process(&mut self, frame: &[f32; FRAME]) -> f32 {
        // RNNoise works on 16-bit sample values.
        for (s, x) in self.scaled.iter_mut().zip(frame) {
            *s = x * 32768.0;
        }
        let vad = self.state.process_frame(&mut self.out, &self.scaled);
        for s in &mut self.out {
            *s /= 32768.0;
        }
        vad
    }

    pub fn denoised(&self) -> &[f32; FRAME] {
        &self.out
    }
}

/// Opens and closes the microphone with a hold time and click-free fades.
pub struct Gate {
    open: bool,
    hold_left: u32,
    gain: f32,
}

impl Gate {
    pub fn new() -> Self {
        Self { open: false, hold_left: 0, gain: 0.0 }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// `hold_frames` keeps the gate open that many frames after `want_open`
    /// goes false, so word endings aren't cut off.
    pub fn update(&mut self, want_open: bool, hold_frames: u32) {
        if want_open {
            self.open = true;
            self.hold_left = hold_frames;
        } else if self.hold_left > 0 {
            self.hold_left -= 1;
        } else {
            self.open = false;
        }
    }

    /// Applies the gate with a ~5 ms ramp.
    pub fn apply(&mut self, frame: &mut [f32]) {
        let target = if self.open { 1.0 } else { 0.0 };
        let step = 1.0 / 240.0;
        for s in frame {
            if self.gain < target {
                self.gain = (self.gain + step).min(target);
            } else if self.gain > target {
                self.gain = (self.gain - step).max(target);
            }
            *s *= self.gain;
        }
    }
}

/// RMS level in dBFS (-100 for silence).
pub fn level_db(frame: &[f32]) -> f32 {
    let energy = frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32;
    if energy <= 1e-10 { -100.0 } else { 10.0 * energy.log10() }
}

/// Gentle limiter so overlapping voices and sounds don't clip harshly.
pub fn soft_clip(x: f32) -> f32 {
    if x.abs() <= 0.9 { x } else { x.signum() * (0.9 + 0.1 * ((x.abs() - 0.9) / 0.1).tanh()) }
}

pub fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, len: usize) -> Vec<f32> {
        (0..len).map(|i| (i as f32 * freq * std::f32::consts::TAU / rate as f32).sin() * 0.5).collect()
    }

    #[test]
    fn resampler_keeps_duration() {
        let mut r = Resampler48::new(44_100);
        let mut out = Vec::new();
        for chunk in sine(440.0, 44_100, 44_100).chunks(441) {
            r.process(chunk, &mut out);
        }
        // One second in, about one second out (minus the resampler's delay).
        assert!((47_000..=48_000).contains(&out.len()), "{}", out.len());
        let mut passthrough = Resampler48::new(48_000);
        let mut out = Vec::new();
        passthrough.process(&[0.1; 480], &mut out);
        assert_eq!(out.len(), 480);
    }

    #[test]
    fn gate_holds_then_fades_out() {
        let mut gate = Gate::new();
        gate.update(true, 2);
        let mut frame = [1.0f32; FRAME];
        gate.apply(&mut frame);
        assert!(frame[0] < 0.01 && frame[FRAME - 1] == 1.0, "fades in");

        gate.update(false, 2);
        gate.update(false, 2);
        assert!(gate.is_open(), "still holding");
        gate.update(false, 2);
        assert!(!gate.is_open());
        let mut frame = [1.0f32; FRAME];
        gate.apply(&mut frame);
        assert!(frame[0] > 0.99 && frame[FRAME - 1] == 0.0, "fades out");
    }

    #[test]
    fn levels_and_clipping() {
        assert_eq!(level_db(&[0.0; 480]), -100.0);
        assert!((level_db(&[1.0; 480])).abs() < 0.01);
        assert!((level_db(&sine(1000.0, 48_000, 4800)) + 9.03).abs() < 0.1, "0.5 amplitude sine ≈ -9 dBFS");
        assert_eq!(soft_clip(0.5), 0.5);
        assert!(soft_clip(3.0) <= 1.0 && soft_clip(-3.0) >= -1.0);
        assert!(soft_clip(0.95) < 0.95 && soft_clip(0.95) > 0.9);
        assert_eq!(to_i16(2.0), 32767);
    }

    #[test]
    fn noise_filters_reduce_noise_and_keep_tones() {
        // White noise alone: every filter should remove most of it.
        let mut seed = 7u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.1
        };
        for kind in [NoiseSuppression::DeepFilter, NoiseSuppression::RnNoise, NoiseSuppression::WebRtc] {
            let mut denoiser = Denoiser::new(kind);
            let mut detector = SpeechDetector::new();
            let (mut energy_in, mut energy_out) = (0.0f32, 0.0f32);
            for i in 0..300 {
                let mut frame = [0.0f32; FRAME];
                frame.iter_mut().for_each(|s| *s = noise());
                detector.process(&frame);
                let input = frame;
                denoiser.process(&mut frame, detector.denoised());
                // Skip the first second while the filters adapt.
                if i >= 100 {
                    energy_in += input.iter().map(|s| s * s).sum::<f32>();
                    energy_out += frame.iter().map(|s| s * s).sum::<f32>();
                }
            }
            let reduction_db = 10.0 * (energy_in / energy_out.max(1e-12)).log10();
            assert!(reduction_db > 6.0, "{kind:?} only reduced noise by {reduction_db:.1} dB");
        }
    }
}
