//! Short UI sounds, synthesized at startup so the binary carries no audio files.

use std::sync::{Arc, OnceLock};

use super::dsp::RATE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    Join,
    Leave,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
    Message,
    ShareStart,
    /// Incoming call; the app repeats it until answered.
    Ring,
}

/// A sequence of notes (frequency in Hz, duration in ms), soft sine with a
/// quick attack and exponential decay per note.
fn chime(notes: &[(f32, u32)], volume: f32) -> Arc<[f32]> {
    let mut out = Vec::new();
    for &(freq, ms) in notes {
        let len = (RATE as usize * ms as usize) / 1000;
        for i in 0..len {
            let t = i as f32 / RATE as f32;
            let attack = (i as f32 / (RATE as f32 * 0.004)).min(1.0);
            let decay = (-t * 9.0).exp();
            // A quiet octave on top makes the tone rounder than a pure sine.
            let tone = (t * freq * std::f32::consts::TAU).sin() + 0.25 * (t * freq * 2.0 * std::f32::consts::TAU).sin();
            out.push(tone * attack * decay * volume);
        }
    }
    // Short fade so the last note never ends in a click.
    let fade = (RATE as usize / 200).min(out.len());
    let n = out.len();
    for (k, s) in out[n - fade..].iter_mut().enumerate() {
        *s *= 1.0 - k as f32 / fade as f32;
    }
    out.into()
}

pub fn samples(sound: Sound) -> Arc<[f32]> {
    static CACHE: OnceLock<Vec<(Sound, Arc<[f32]>)>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        vec![
            (Sound::Join, chime(&[(659.3, 90), (987.8, 220)], 0.22)),
            (Sound::Leave, chime(&[(987.8, 90), (659.3, 220)], 0.22)),
            (Sound::Mute, chime(&[(523.3, 70), (392.0, 120)], 0.18)),
            (Sound::Unmute, chime(&[(392.0, 70), (523.3, 120)], 0.18)),
            (Sound::Deafen, chime(&[(440.0, 80), (329.6, 80), (261.6, 140)], 0.18)),
            (Sound::Undeafen, chime(&[(261.6, 80), (329.6, 80), (440.0, 140)], 0.18)),
            (Sound::Message, chime(&[(1318.5, 160)], 0.12)),
            (Sound::ShareStart, chime(&[(523.3, 70), (659.3, 70), (784.0, 180)], 0.18)),
            (Sound::Ring, chime(&[(784.0, 120), (987.8, 120), (784.0, 120), (987.8, 260)], 0.2)),
        ]
    });
    cache.iter().find(|(s, _)| *s == sound).map(|(_, samples)| samples.clone()).expect("every sound is synthesized")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_are_short_and_never_clip() {
        for sound in [
            Sound::Join,
            Sound::Leave,
            Sound::Mute,
            Sound::Unmute,
            Sound::Deafen,
            Sound::Undeafen,
            Sound::Message,
            Sound::ShareStart,
            Sound::Ring,
        ] {
            let s = samples(sound);
            assert!(s.len() < RATE as usize, "{sound:?} shorter than a second");
            assert!(s.iter().all(|x| x.abs() < 0.5), "{sound:?} stays quiet");
            assert!(s.last().unwrap().abs() < 1e-3, "{sound:?} ends silent");
        }
    }
}
