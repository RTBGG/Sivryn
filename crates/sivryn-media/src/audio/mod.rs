//! Sivryn's own audio engine, replacing WebRTC's audio device module so the
//! microphone chain and playback are under our control.
//!
//! ```text
//! mic ─ cpal ─ resample 48k ─ gain ─ HPF+AEC3 ─ noise filter ─ AGC2 ─ gate ─ LiveKit
//!                                       ▲
//! LiveKit ─ per-user volume ─ mixer ─ cpal ─ speakers/headset
//!           sounds, mic test ─┘   └─ far-end reference for AEC3
//! ```
//!
//! Device callbacks only touch lock-free ring buffers; processing and mixing
//! run on two dedicated threads that exist only while something needs them
//! (a call, the mic test, a sound).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use futures::StreamExt;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};

mod config;
pub(crate) mod dsp;
mod sounds;

pub use config::{AudioConfig, NoiseSuppression, VoiceMode};
use dsp::{FRAME, RATE};
pub use sounds::Sound;

/// Remote audio is kept at most this far behind (samples, stereo).
const MAX_REMOTE_BUFFER: usize = (RATE as usize / 1000) * 150 * 2;
/// …and trimmed back to this when it falls further behind.
const TRIMMED_REMOTE_BUFFER: usize = (RATE as usize / 1000) * 40 * 2;
/// A remote source starts (or restarts after running dry) once this much is buffered.
const PRIME_REMOTE_BUFFER: usize = (RATE as usize / 1000) * 20 * 2;
/// How much mixed audio waits for the output device.
const OUTPUT_TARGET: usize = (RATE as usize / 1000) * 30 * 2;
/// Voice activity hold time (frames of 10 ms).
const VOICE_HOLD_FRAMES: u32 = 30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

fn list(devices: Result<impl Iterator<Item = cpal::Device>, cpal::Error>) -> Vec<AudioDevice> {
    let Ok(devices) = devices else { return Vec::new() };
    devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d.description().map(|desc| desc.name().to_owned()).unwrap_or_else(|_| d.to_string());
            Some(AudioDevice { id, name })
        })
        .collect()
}

pub fn input_devices() -> Vec<AudioDevice> {
    list(cpal::default_host().input_devices())
}

pub fn output_devices() -> Vec<AudioDevice> {
    list(cpal::default_host().output_devices())
}

fn find_device(id: Option<&str>, input: bool) -> Option<cpal::Device> {
    let host = cpal::default_host();
    let by_id = id.and_then(|id| id.parse().ok()).and_then(|id| host.device_by_id(&id));
    by_id.or_else(|| if input { host.default_input_device() } else { host.default_output_device() })
}

/// One remote audio track (a microphone or a screen share's audio).
struct Remote {
    track_sid: String,
    /// Volume key: the participant identity, plus `:screen` for share audio.
    volume_key: String,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    primed: bool,
    /// Feeds `buffer` from a LiveKit track; `None` for local streams.
    task: Option<tokio::task::JoinHandle<()>>,
}

struct Inner {
    config: Mutex<AudioConfig>,
    config_version: AtomicU64,
    muted: AtomicBool,
    deafened: AtomicBool,
    push_to_talk: AtomicBool,
    mic_test: AtomicBool,
    in_call: AtomicBool,
    /// Processed microphone level in dBFS (f32 bits), for meters.
    level: AtomicU32,
    transmitting: AtomicBool,
    mic: Mutex<Option<NativeAudioSource>>,
    remotes: Mutex<Vec<Remote>>,
    volumes: Mutex<HashMap<String, f32>>,
    sounds: Mutex<Vec<(Arc<[f32]>, usize)>>,
    /// Mic test: processed microphone audio played back to the user.
    monitor: Mutex<VecDeque<f32>>,
    /// Shared by capture (near end) and playout (far end reference).
    echo: Mutex<sonora::AudioProcessing>,
    capture_running: AtomicBool,
    playout_running: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicked audio thread must not take the whole engine down with it.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inner {
    fn capture_wanted(&self) -> bool {
        lock(&self.mic).is_some() || self.mic_test.load(Ordering::Relaxed)
    }

    fn playout_wanted(&self) -> bool {
        self.in_call.load(Ordering::Relaxed)
            || self.mic_test.load(Ordering::Relaxed)
            || !lock(&self.remotes).is_empty()
            || !lock(&self.sounds).is_empty()
    }
}

/// Handle to the process-wide audio engine. Cheap to clone.
#[derive(Clone)]
pub struct AudioEngine {
    inner: Arc<Inner>,
}

impl AudioEngine {
    pub fn new(config: AudioConfig) -> Self {
        let echo = dsp::echo_canceller(config.echo_cancellation);
        Self {
            inner: Arc::new(Inner {
                config: Mutex::new(config),
                config_version: AtomicU64::new(0),
                muted: AtomicBool::new(false),
                deafened: AtomicBool::new(false),
                push_to_talk: AtomicBool::new(false),
                mic_test: AtomicBool::new(false),
                in_call: AtomicBool::new(false),
                level: AtomicU32::new((-100.0f32).to_bits()),
                transmitting: AtomicBool::new(false),
                mic: Mutex::new(None),
                remotes: Mutex::new(Vec::new()),
                volumes: Mutex::new(HashMap::new()),
                sounds: Mutex::new(Vec::new()),
                monitor: Mutex::new(VecDeque::new()),
                echo: Mutex::new(echo),
                capture_running: AtomicBool::new(false),
                playout_running: AtomicBool::new(false),
            }),
        }
    }

    pub fn config(&self) -> AudioConfig {
        lock(&self.inner.config).clone()
    }

    /// Applies new settings; running streams pick them up within 10 ms
    /// (device changes reopen the stream).
    pub fn set_config(&self, config: AudioConfig) {
        *lock(&self.inner.config) = config;
        self.inner.config_version.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_muted(&self, muted: bool) {
        self.inner.muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.inner.deafened.store(deafened, Ordering::Relaxed);
    }

    pub fn set_push_to_talk(&self, pressed: bool) {
        self.inner.push_to_talk.store(pressed, Ordering::Relaxed);
    }

    /// Volume for a participant (`identity`) or their screen share audio
    /// (`identity:screen`), 0.0–2.0.
    pub fn set_volume(&self, key: &str, volume: f32) {
        lock(&self.inner.volumes).insert(key.to_owned(), volume.clamp(0.0, 2.0));
    }

    pub fn set_volumes(&self, volumes: HashMap<String, f32>) {
        *lock(&self.inner.volumes) = volumes;
    }

    /// Processed microphone level in dBFS, for meters.
    pub fn level_db(&self) -> f32 {
        f32::from_bits(self.inner.level.load(Ordering::Relaxed))
    }

    /// Whether the microphone is currently sending voice.
    pub fn transmitting(&self) -> bool {
        self.inner.transmitting.load(Ordering::Relaxed)
    }

    /// Mic test: runs the full chain and plays it back to the user.
    pub fn set_mic_test(&self, on: bool) {
        self.inner.mic_test.store(on, Ordering::Relaxed);
        if !on {
            lock(&self.inner.monitor).clear();
        }
        self.ensure_threads();
    }

    pub fn play(&self, sound: Sound) {
        if !lock(&self.inner.config).sounds {
            return;
        }
        lock(&self.inner.sounds).push((sounds::samples(sound), 0));
        self.ensure_threads();
    }

    pub(crate) fn set_in_call(&self, in_call: bool) {
        self.inner.in_call.store(in_call, Ordering::Relaxed);
        if !in_call {
            self.inner.transmitting.store(false, Ordering::Relaxed);
        }
        self.ensure_threads();
    }

    pub(crate) fn attach_microphone(&self, source: Option<NativeAudioSource>) {
        *lock(&self.inner.mic) = source;
        self.ensure_threads();
    }

    /// Starts playing a remote track. Must be called inside the Tokio runtime.
    pub(crate) fn add_remote(&self, track_sid: String, volume_key: String, mut stream: NativeAudioStream) {
        let buffer = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_REMOTE_BUFFER * 2)));
        let sink = buffer.clone();
        let sid = track_sid.clone();
        let task = tokio::spawn(async move {
            let mut first = true;
            while let Some(frame) = stream.next().await {
                if first {
                    tracing::info!(track = %sid, rate = frame.sample_rate, channels = frame.num_channels, "first remote audio frame");
                    first = false;
                }
                let mut buf = lock(&sink);
                // The mixer trims excess latency; this only bounds memory if it stalls.
                if buf.len() < MAX_REMOTE_BUFFER * 4 {
                    buf.extend(frame.data.iter().map(|&s| s as f32 / 32768.0));
                }
            }
        });
        let mut remotes = lock(&self.inner.remotes);
        remotes.retain(|r| {
            let keep = r.track_sid != track_sid;
            if !keep && let Some(task) = &r.task {
                task.abort();
            }
            keep
        });
        remotes.push(Remote { track_sid, volume_key, buffer, primed: false, task: Some(task) });
        drop(remotes);
        self.ensure_threads();
    }

    /// Plays audio produced locally (a video in the chat): push interleaved
    /// 48 kHz stereo samples into the returned buffer. Remove it with
    /// [`AudioEngine::remove_local_stream`].
    pub fn add_local_stream(&self, key: &str) -> Arc<Mutex<VecDeque<f32>>> {
        let buffer = Arc::new(Mutex::new(VecDeque::new()));
        lock(&self.inner.remotes).push(Remote {
            track_sid: key.to_owned(),
            volume_key: key.to_owned(),
            buffer: buffer.clone(),
            primed: false,
            task: None,
        });
        self.ensure_threads();
        buffer
    }

    pub fn remove_local_stream(&self, key: &str) {
        self.remove_remote(key);
    }

    pub(crate) fn remove_remote(&self, track_sid: &str) {
        lock(&self.inner.remotes).retain(|r| {
            let keep = r.track_sid != track_sid;
            if !keep && let Some(task) = &r.task {
                task.abort();
            }
            keep
        });
    }

    pub(crate) fn clear_remotes(&self) {
        for r in lock(&self.inner.remotes).drain(..) {
            if let Some(task) = &r.task {
                task.abort();
            }
        }
    }

    fn ensure_threads(&self) {
        let inner = &self.inner;
        if inner.capture_wanted() && !inner.capture_running.swap(true, Ordering::AcqRel) {
            let inner = inner.clone();
            spawn_named("sivryn-audio-capture", move || {
                supervise(&inner, &inner.capture_running, Inner::capture_wanted, run_capture)
            });
        }
        if inner.playout_wanted() && !inner.playout_running.swap(true, Ordering::AcqRel) {
            let inner = inner.clone();
            spawn_named("sivryn-audio-playout", move || {
                supervise(&inner, &inner.playout_running, Inner::playout_wanted, run_playout)
            });
        }
    }
}

fn spawn_named(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        tracing::error!(error = %e, name, "could not start audio thread");
    }
}

/// Runs `body` while `wanted`, without losing a wake-up that races with the
/// thread deciding to exit.
fn supervise(inner: &Arc<Inner>, running: &AtomicBool, wanted: fn(&Inner) -> bool, body: fn(&Arc<Inner>)) {
    loop {
        body(inner);
        running.store(false, Ordering::Release);
        if !wanted(inner) || running.swap(true, Ordering::AcqRel) {
            return;
        }
    }
}

// --- Capture ---------------------------------------------------------------

/// Whether a stream error means the stream must be reopened. Buffer
/// over/underruns (reported since cpal 0.18) are momentary glitches.
fn stream_broken(e: &cpal::Error) -> bool {
    !matches!(e.kind(), cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied)
}

struct InputStream {
    _stream: cpal::Stream,
    samples: ringbuf::HeapCons<f32>,
    failed: Arc<AtomicBool>,
    resampler: dsp::Resampler48,
}

fn open_input(device_id: Option<&str>) -> Result<InputStream, String> {
    let device = find_device(device_id, true).ok_or("no microphone found")?;
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let channels = supported.channels() as usize;
    let rate = supported.sample_rate();
    let config: cpal::StreamConfig = supported.config();

    let (mut producer, consumer) = HeapRb::<f32>::new(rate as usize).split();
    let failed = Arc::new(AtomicBool::new(false));
    let on_error = {
        let failed = failed.clone();
        move |e: cpal::Error| {
            if stream_broken(&e) {
                tracing::warn!(error = %e, "microphone stream error");
                failed.store(true, Ordering::Relaxed);
            }
        }
    };
    let waker = std::thread::current();

    macro_rules! build {
        ($t:ty, $to_f32:expr) => {
            device.build_input_stream::<$t, _, _>(
                config,
                move |data: &[$t], _| {
                    // Downmix to mono: voice needs one channel.
                    for frame in data.chunks_exact(channels) {
                        let sum: f32 = frame.iter().map(|&s| $to_f32(s)).sum();
                        let _ = producer.try_push(sum / channels as f32);
                    }
                    waker.unpark();
                },
                on_error,
                None,
            )
        };
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build!(f32, |s: f32| s),
        cpal::SampleFormat::I16 => build!(i16, |s: i16| s as f32 / 32768.0),
        cpal::SampleFormat::I32 => build!(i32, |s: i32| s as f32 / 2_147_483_648.0),
        cpal::SampleFormat::U16 => build!(u16, |s: u16| (s as f32 - 32768.0) / 32768.0),
        other => return Err(format!("unsupported microphone sample format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    tracing::info!(name = %device, rate, channels, "microphone opened");
    Ok(InputStream { _stream: stream, samples: consumer, failed, resampler: dsp::Resampler48::new(rate) })
}

struct CaptureChain {
    config: AudioConfig,
    denoiser_kind: Option<NoiseSuppression>,
    denoiser: dsp::Denoiser,
    detector: dsp::SpeechDetector,
    agc: sonora::AudioProcessing,
    gate: dsp::Gate,
    echo_enabled: Option<bool>,
}

fn run_capture(inner: &Arc<Inner>) {
    let mut version = u64::MAX;
    let mut input: Option<InputStream> = None;
    let mut input_device: Option<Option<String>> = None;
    let mut retry_at = Instant::now();
    let mut chain = CaptureChain {
        config: AudioConfig::default(),
        denoiser_kind: None,
        denoiser: dsp::Denoiser::Off,
        detector: dsp::SpeechDetector::new(),
        agc: dsp::auto_gain(),
        gate: dsp::Gate::new(),
        echo_enabled: None,
    };
    let mut raw = Vec::new();
    let mut pcm = Vec::new();

    while inner.capture_wanted() {
        let v = inner.config_version.load(Ordering::Relaxed);
        if v != version {
            version = v;
            chain.config = lock(&inner.config).clone();
            if chain.denoiser_kind != Some(chain.config.noise_suppression) {
                chain.denoiser = dsp::Denoiser::new(chain.config.noise_suppression);
                chain.denoiser_kind = Some(chain.config.noise_suppression);
            }
            if chain.echo_enabled != Some(chain.config.echo_cancellation) {
                *lock(&inner.echo) = dsp::echo_canceller(chain.config.echo_cancellation);
                chain.echo_enabled = Some(chain.config.echo_cancellation);
            }
            if input_device.as_ref() != Some(&chain.config.input_device) {
                input = None;
            }
        }

        if input.as_ref().is_some_and(|i| i.failed.load(Ordering::Relaxed)) {
            input = None;
        }
        if input.is_none() && Instant::now() >= retry_at {
            match open_input(chain.config.input_device.as_deref()) {
                Ok(stream) => {
                    input = Some(stream);
                    input_device = Some(chain.config.input_device.clone());
                }
                Err(e) => {
                    tracing::warn!(error = %e, "could not open microphone; retrying");
                    retry_at = Instant::now() + Duration::from_secs(2);
                }
            }
        }

        std::thread::park_timeout(Duration::from_millis(5));
        if let Some(input) = &mut input {
            raw.resize(input.samples.occupied_len(), 0.0);
            let n = input.samples.pop_slice(&mut raw);
            input.resampler.process(&raw[..n], &mut pcm);
        }
        let mut offset = 0;
        while pcm.len() - offset >= FRAME {
            let mut frame = [0.0f32; FRAME];
            frame.copy_from_slice(&pcm[offset..offset + FRAME]);
            offset += FRAME;
            process_frame(inner, &mut chain, &mut frame);
        }
        pcm.drain(..offset);
    }
    inner.transmitting.store(false, Ordering::Relaxed);
    inner.level.store((-100.0f32).to_bits(), Ordering::Relaxed);
}

fn process_frame(inner: &Inner, chain: &mut CaptureChain, frame: &mut [f32; FRAME]) {
    let config = &chain.config;
    for s in frame.iter_mut() {
        *s *= config.input_volume;
    }
    let input = *frame;
    let _ = lock(&inner.echo).process_capture_f32(&[&input], &mut [&mut frame[..]]);

    let speech = chain.detector.process(frame);
    chain.denoiser.process(frame, chain.detector.denoised());
    if config.auto_gain {
        let input = *frame;
        let _ = chain.agc.process_capture_f32(&[&input], &mut [&mut frame[..]]);
    }
    let level = dsp::level_db(frame);
    inner.level.store(level.to_bits(), Ordering::Relaxed);

    let (want, hold) = match config.voice_mode {
        VoiceMode::VoiceActivity if config.auto_sensitivity => {
            // Hysteresis: start on clear speech, keep going through softer parts.
            let threshold = if chain.gate.is_open() { 0.4 } else { 0.75 };
            (speech > threshold, VOICE_HOLD_FRAMES)
        }
        VoiceMode::VoiceActivity => (level > config.sensitivity_db, VOICE_HOLD_FRAMES),
        VoiceMode::PushToTalk => (inner.push_to_talk.load(Ordering::Relaxed), config.ptt_release_ms / 10),
    };
    let silenced = inner.muted.load(Ordering::Relaxed) || inner.deafened.load(Ordering::Relaxed);
    chain.gate.update(want && !silenced, if silenced { 0 } else { hold });
    chain.gate.apply(frame);
    inner.transmitting.store(chain.gate.is_open(), Ordering::Relaxed);

    if inner.mic_test.load(Ordering::Relaxed) {
        let mut monitor = lock(&inner.monitor);
        if monitor.len() > FRAME * 10 {
            monitor.drain(..FRAME);
        }
        monitor.extend(frame.iter());
    }

    let mic = lock(&inner.mic).clone();
    if let Some(source) = mic {
        let data: Vec<i16> = frame.iter().map(|&s| dsp::to_i16(s)).collect();
        let audio =
            AudioFrame { data: data.into(), sample_rate: RATE, num_channels: 1, samples_per_channel: FRAME as u32 };
        if let Err(e) = futures::executor::block_on(source.capture_frame(&audio)) {
            tracing::trace!(error = %e, "dropped microphone frame");
        }
    }
}

// --- Playout ---------------------------------------------------------------

struct OutputStream {
    _stream: cpal::Stream,
    samples: ringbuf::HeapProd<f32>,
    failed: Arc<AtomicBool>,
}

fn open_output(device_id: Option<&str>) -> Result<OutputStream, String> {
    let device = find_device(device_id, false).ok_or("no speakers or headset found")?;
    let channels = device.default_output_config().map(|c| c.channels()).unwrap_or(2).max(1);
    // WASAPI converts the sample rate for output streams, so mixing always
    // happens at 48 kHz stereo.
    let config = cpal::StreamConfig { channels, sample_rate: RATE, buffer_size: cpal::BufferSize::Default };
    let (producer, mut consumer) = HeapRb::<f32>::new(RATE as usize).split();
    let failed = Arc::new(AtomicBool::new(false));
    let on_error = {
        let failed = failed.clone();
        move |e: cpal::Error| {
            if stream_broken(&e) {
                tracing::warn!(error = %e, "speaker stream error");
                failed.store(true, Ordering::Relaxed);
            }
        }
    };
    let out_channels = channels as usize;
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |data: &mut [f32], _| {
                for frame in data.chunks_exact_mut(out_channels) {
                    let (l, r) = if consumer.occupied_len() >= 2 {
                        (consumer.try_pop().unwrap_or(0.0), consumer.try_pop().unwrap_or(0.0))
                    } else {
                        (0.0, 0.0)
                    };
                    if out_channels == 1 {
                        frame[0] = (l + r) * 0.5;
                    } else {
                        frame[0] = l;
                        frame[1] = r;
                        frame[2..].fill(0.0);
                    }
                }
            },
            on_error,
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    tracing::info!(name = %device, channels, "audio output opened");
    Ok(OutputStream { _stream: stream, samples: producer, failed })
}

fn run_playout(inner: &Arc<Inner>) {
    let mut version = u64::MAX;
    let mut config = AudioConfig::default();
    let mut output: Option<OutputStream> = None;
    let mut output_device: Option<Option<String>> = None;
    let mut retry_at = Instant::now();
    let mut idle_since: Option<Instant> = None;
    let mut mix = [0.0f32; FRAME * 2];

    loop {
        // Linger briefly so the end of a sound isn't cut off.
        if inner.playout_wanted() {
            idle_since = None;
        } else if idle_since.get_or_insert_with(Instant::now).elapsed() > Duration::from_millis(500) {
            break;
        }

        let v = inner.config_version.load(Ordering::Relaxed);
        if v != version {
            version = v;
            config = lock(&inner.config).clone();
            if output_device.as_ref() != Some(&config.output_device) {
                output = None;
            }
        }
        if output.as_ref().is_some_and(|o| o.failed.load(Ordering::Relaxed)) {
            output = None;
        }
        if output.is_none() && Instant::now() >= retry_at {
            match open_output(config.output_device.as_deref()) {
                Ok(stream) => {
                    output = Some(stream);
                    output_device = Some(config.output_device.clone());
                }
                Err(e) => {
                    tracing::warn!(error = %e, "could not open audio output; retrying");
                    retry_at = Instant::now() + Duration::from_secs(2);
                }
            }
        }

        match &mut output {
            Some(out) => {
                while out.samples.occupied_len() < OUTPUT_TARGET && out.samples.vacant_len() >= mix.len() {
                    mix_frame(inner, &config, &mut mix);
                    out.samples.push_slice(&mix);
                }
            }
            // Without a device, keep consuming so buffers don't pile up.
            None => mix_frame(inner, &config, &mut mix),
        }
        std::thread::sleep(Duration::from_millis(3));
    }
}

fn mix_frame(inner: &Inner, config: &AudioConfig, mix: &mut [f32; FRAME * 2]) {
    mix.fill(0.0);
    let master = config.output_volume;

    if !inner.deafened.load(Ordering::Relaxed) {
        let volumes = lock(&inner.volumes).clone();
        for remote in lock(&inner.remotes).iter_mut() {
            let mut buf = lock(&remote.buffer);
            if !remote.primed {
                if buf.len() < PRIME_REMOTE_BUFFER {
                    continue;
                }
                remote.primed = true;
            }
            if buf.len() > MAX_REMOTE_BUFFER {
                let excess = (buf.len() - TRIMMED_REMOTE_BUFFER) & !1;
                buf.drain(..excess);
            }
            let volume = master * volumes.get(&remote.volume_key).copied().unwrap_or(1.0);
            for out in mix.iter_mut() {
                match buf.pop_front() {
                    Some(s) => *out += s * volume,
                    None => {
                        remote.primed = false;
                        break;
                    }
                }
            }
        }
    }

    if inner.mic_test.load(Ordering::Relaxed) {
        let mut monitor = lock(&inner.monitor);
        for pair in mix.as_chunks_mut::<2>().0 {
            let s = monitor.pop_front().unwrap_or(0.0) * master;
            pair[0] += s;
            pair[1] += s;
        }
    }

    {
        let mut sounds = lock(&inner.sounds);
        let volume = master.min(1.0);
        for (samples, pos) in sounds.iter_mut() {
            for pair in mix.as_chunks_mut::<2>().0 {
                let Some(&s) = samples.get(*pos) else { break };
                pair[0] += s * volume;
                pair[1] += s * volume;
                *pos += 1;
            }
        }
        sounds.retain(|(samples, pos)| *pos < samples.len());
    }

    for s in mix.iter_mut() {
        *s = dsp::soft_clip(*s);
    }

    // What the speakers play is what AEC3 must remove from the microphone.
    let mut reference = [0.0f32; FRAME];
    for (r, pair) in reference.iter_mut().zip(mix.as_chunks::<2>().0) {
        *r = (pair[0] + pair[1]) * 0.5;
    }
    let input = reference;
    let _ = lock(&inner.echo).process_render_f32(&[&input], &mut [&mut reference[..]]);
}
