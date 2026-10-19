//! Checks that screen share audio leaves out Sivryn's own playback.
//!
//! Plays a quiet 440 Hz tone from this process while capturing system audio
//! twice: once *excluding* this process (what a screen share sends) and once
//! *only* this process (control). Reports the 440 Hz level of each:
//! `cargo run -p sivryn-media --example loopback`.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sivryn_media::LoopbackTarget;
use sivryn_media::screen_audio::capture_probe;

fn main() {
    let device = cpal::default_host().default_output_device().expect("an output device");
    let config = cpal::StreamConfig { channels: 2, sample_rate: 48_000, buffer_size: cpal::BufferSize::Default };
    let mut phase = 0.0f32;
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |data: &mut [f32], _| {
                for frame in data.as_chunks_mut::<2>().0 {
                    let s = (phase * std::f32::consts::TAU).sin() * 0.05;
                    phase = (phase + 440.0 / 48_000.0).fract();
                    frame[0] = s;
                    frame[1] = s;
                }
            },
            |e| eprintln!("output error: {e}"),
            None,
        )
        .expect("output stream");
    stream.play().expect("play");

    let me = std::process::id();
    for (label, target) in
        [("excluding this process", LoopbackTarget::AllExcept(me)), ("only this process", LoopbackTarget::Process(me))]
    {
        let samples = capture_probe(target, 2).expect("capture");
        // Skip the first 0.5 s while capture settles; left channel only.
        let left: Vec<f32> = samples.iter().step_by(2).skip(24_000).map(|&s| s as f32 / 32768.0).collect();
        println!("{label:24} 440 Hz level: {:>7.1} dBFS ({} samples)", tone_db(&left, 440.0), left.len());
    }
}

/// Level of one frequency (Goertzel), in dBFS of a full-scale sine.
fn tone_db(x: &[f32], freq: f32) -> f32 {
    if x.is_empty() {
        return -120.0;
    }
    let w = std::f32::consts::TAU * freq / 48_000.0;
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &v in x {
        let s0 = v + 2.0 * w.cos() * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2;
    let amplitude = 2.0 * power.sqrt() / x.len() as f32;
    20.0 * amplitude.max(1e-6).log10()
}
