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
    ShareStop,
    /// Incoming call; the app repeats it until answered.
    Ring,
}

/// A sequence of notes (frequency in Hz, duration in ms), soft sine with a
/// gentle attack and exponential decay per note. Each note rings on under
/// the next one and fades out smoothly instead of stopping mid-wave, which
/// is what made the old chimes click between notes.
fn chime(notes: &[(f32, u32)], volume: f32) -> Arc<[f32]> {
    const ATTACK_S: f32 = 0.008;
    const RING_S: f32 = 0.06;
    let total: usize = notes.iter().map(|&(_, ms)| RATE as usize * ms as usize / 1000).sum();
    let mut out = vec![0.0f32; total + (RATE as f32 * RING_S) as usize];
    let mut start = 0;
    for &(freq, ms) in notes {
        let len = RATE as usize * ms as usize / 1000;
        let ring = (RATE as f32 * RING_S) as usize;
        for i in 0..(len + ring).min(out.len() - start) {
            let t = i as f32 / RATE as f32;
            // Raised-cosine attack and release: smooth at both ends.
            let attack = if t < ATTACK_S { 0.5 - 0.5 * (std::f32::consts::PI * t / ATTACK_S).cos() } else { 1.0 };
            let release =
                if i < len { 1.0 } else { 0.5 + 0.5 * (std::f32::consts::PI * (i - len) as f32 / ring as f32).cos() };
            let decay = (-t * 9.0).exp();
            // A quiet octave on top makes the tone rounder than a pure sine.
            let tone = (t * freq * std::f32::consts::TAU).sin() + 0.25 * (t * freq * 2.0 * std::f32::consts::TAU).sin();
            out[start + i] += tone * attack * release * decay * volume;
        }
        start += len;
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
            (Sound::ShareStop, chime(&[(784.0, 70), (659.3, 70), (523.3, 180)], 0.18)),
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
            Sound::ShareStop,
            Sound::Ring,
        ] {
            let s = samples(sound);
            assert!(s.len() < RATE as usize, "{sound:?} shorter than a second");
            assert!(s.iter().all(|x| x.abs() < 0.5), "{sound:?} stays quiet");
            assert!(s.last().unwrap().abs() < 1e-3, "{sound:?} ends silent");
            assert!(s[0].abs() < 1e-3, "{sound:?} starts silent");
            // No jumps between neighbouring samples (what clicks sound like).
            let max_step = s.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
            // A pure tone changes by at most ~0.05 per sample; a click jumps by its full amplitude.
            assert!(max_step < 0.08, "{sound:?} jumps by {max_step}");
        }
    }
}
