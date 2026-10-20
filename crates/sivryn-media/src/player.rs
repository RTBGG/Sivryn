//! Plays a video (or audio) file from memory with Windows Media Foundation:
//! hardware-accelerated decoding, frames as RGBA for the UI's GPU surface,
//! sound through Sivryn's audio engine. Decrypted attachments never touch
//! the disk.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use tokio::sync::watch;

use crate::MediaError;
use crate::audio::AudioEngine;
use crate::video::RgbaFrame;

#[derive(Default)]
struct Shared {
    playing: AtomicBool,
    stop: AtomicBool,
    ended: AtomicBool,
    position_ms: AtomicU64,
    duration_ms: AtomicU64,
    seek_to: Mutex<Option<u64>>,
}

/// A playing (or paused) media file. Dropping it stops playback.
pub struct MediaPlayer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    pub frames: watch::Receiver<Option<Arc<RgbaFrame>>>,
    engine: AudioEngine,
    audio_key: String,
}

impl MediaPlayer {
    /// Starts decoding `data` (an MP4, MOV, WebM, MP3 … file) and plays it.
    pub fn open(data: Arc<Vec<u8>>, engine: AudioEngine) -> Result<Self, MediaError> {
        if !crate::media_foundation_available() {
            return Err(MediaError::NoMediaFoundation);
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let audio_key = format!("player:{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let audio = engine.add_local_stream(&audio_key);
        let shared = Arc::new(Shared::default());
        shared.playing.store(true, Ordering::Relaxed);
        let (tx, frames) = watch::channel(None);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("sivryn-player".into())
            .spawn(move || {
                #[cfg(windows)]
                imp::run(data, thread_shared, tx, audio, ready_tx);
                #[cfg(not(windows))]
                {
                    let _ = (data, thread_shared, tx, audio);
                    let _ = ready_tx.send(Err(MediaError::Audio("video playback needs Windows".into())));
                }
            })
            .map_err(|e| MediaError::Audio(e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = thread.join();
                engine.remove_local_stream(&audio_key);
                return Err(e);
            }
            Err(_) => {
                engine.remove_local_stream(&audio_key);
                return Err(MediaError::Audio("player thread exited".into()));
            }
        }
        Ok(Self { shared, thread: Some(thread), frames, engine, audio_key })
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    pub fn set_playing(&self, playing: bool) {
        // Playing again after the end starts over.
        if playing && self.shared.ended.swap(false, Ordering::Relaxed) {
            self.seek(0);
        }
        self.shared.playing.store(playing, Ordering::Relaxed);
    }

    pub fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::Relaxed)
    }

    pub fn seek(&self, ms: u64) {
        *self.shared.seek_to.lock().unwrap_or_else(|e| e.into_inner()) = Some(ms);
        self.shared.ended.store(false, Ordering::Relaxed);
    }

    pub fn position_ms(&self) -> u64 {
        self.shared.position_ms.load(Ordering::Relaxed)
    }

    pub fn duration_ms(&self) -> u64 {
        self.shared.duration_ms.load(Ordering::Relaxed)
    }
}

impl Drop for MediaPlayer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.engine.remove_local_stream(&self.audio_key);
    }
}

#[cfg(windows)]
mod imp {
    use std::collections::VecDeque;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use tokio::sync::watch;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::StructuredStorage::{
        PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
    };
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::System::Variant::VT_I8;
    use windows::Win32::UI::Shell::SHCreateMemStream;
    use windows::core::GUID;

    use super::Shared;
    use crate::MediaError;
    use crate::video::RgbaFrame;

    const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    const ANY: u32 = MF_SOURCE_READER_ANY_STREAM.0 as u32;
    /// Audio is decoded this far ahead of the clock, so the mixer never runs dry.
    const AUDIO_LEAD_MS: i64 = 80;

    struct Format {
        width: u32,
        height: u32,
        rate: u32,
        channels: u32,
    }

    fn open_reader(data: &[u8]) -> windows::core::Result<(IMFSourceReader, bool, bool)> {
        // SAFETY: plain Media Foundation setup on this (MTA) thread.
        unsafe {
            let stream = SHCreateMemStream(Some(data))
                .ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_OUTOFMEMORY))?;
            let byte_stream = MFCreateMFByteStreamOnStream(&stream)?;
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 2)?;
            let attrs = attrs.expect("attributes created");
            // Lets the reader convert decoded frames to RGB32 (on the GPU when possible).
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
            let reader = MFCreateSourceReaderFromByteStream(&byte_stream, &attrs)?;

            let video = MFCreateMediaType()?;
            video.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            video.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            let has_video = reader.SetCurrentMediaType(VIDEO, None, &video).is_ok();

            let audio = MFCreateMediaType()?;
            audio.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            audio.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
            let has_audio = reader.SetCurrentMediaType(AUDIO, None, &audio).is_ok();
            Ok((reader, has_video, has_audio))
        }
    }

    fn read_format(reader: &IMFSourceReader, has_video: bool, has_audio: bool) -> Format {
        let mut f = Format { width: 0, height: 0, rate: 48_000, channels: 2 };
        // SAFETY: queries on configured streams.
        unsafe {
            if has_video
                && let Ok(t) = reader.GetCurrentMediaType(VIDEO)
                && let Ok(size) = t.GetUINT64(&MF_MT_FRAME_SIZE)
            {
                f.width = (size >> 32) as u32;
                f.height = size as u32;
            }
            if has_audio && let Ok(t) = reader.GetCurrentMediaType(AUDIO) {
                f.rate = t.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(48_000);
                f.channels = t.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2).max(1);
            }
        }
        f
    }

    fn duration_ms(reader: &IMFSourceReader) -> u64 {
        // SAFETY: PROPVARIANT holds a u64 (VT_UI8) for MF_PD_DURATION.
        unsafe {
            reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .map(|v| v.Anonymous.Anonymous.Anonymous.uhVal / 10_000)
                .unwrap_or(0)
        }
    }

    fn seek(reader: &IMFSourceReader, ms: u64) {
        let position = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_I8,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 { hVal: ms as i64 * 10_000 },
                }),
            },
        };
        // SAFETY: GUID_NULL means 100 ns units.
        if let Err(e) = unsafe { reader.SetCurrentPosition(&GUID::zeroed(), &position) } {
            tracing::debug!(error = %e, "seek failed");
        }
    }

    fn sample_bytes(sample: &IMFSample) -> Option<Vec<u8>> {
        // SAFETY: Lock/Unlock pair around a copy of the buffer contents.
        unsafe {
            let buffer = sample.ConvertToContiguousBuffer().ok()?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            buffer.Lock(&mut ptr, None, Some(&mut len)).ok()?;
            let out = std::slice::from_raw_parts(ptr, len as usize).to_vec();
            let _ = buffer.Unlock();
            Some(out)
        }
    }

    /// RGB32 (B, G, R, X) to RGBA, honouring the row stride.
    fn to_rgba(bytes: &[u8], width: u32, height: u32) -> Option<RgbaFrame> {
        if width == 0 || height == 0 {
            return None;
        }
        let stride = bytes.len() / height as usize;
        if stride < width as usize * 4 {
            return None;
        }
        let mut data = vec![0u8; (width * height * 4) as usize];
        for y in 0..height as usize {
            let src = &bytes[y * stride..y * stride + width as usize * 4];
            let dst = &mut data[y * width as usize * 4..(y + 1) * width as usize * 4];
            for (s, d) in src.as_chunks::<4>().0.iter().zip(dst.as_chunks_mut::<4>().0.iter_mut()) {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = 255;
            }
        }
        Some(RgbaFrame { width, height, data })
    }

    /// Float samples in the file's format to interleaved 48 kHz stereo.
    fn to_stereo_48k(samples: &[f32], rate: u32, channels: u32, out: &mut VecDeque<f32>) {
        let ch = channels as usize;
        let frames = samples.len() / ch;
        let pick = |i: usize, c: usize| samples[i * ch + c.min(ch - 1)];
        if rate == 48_000 {
            for i in 0..frames {
                out.push_back(pick(i, 0));
                out.push_back(pick(i, 1));
            }
            return;
        }
        // Linear interpolation is fine for clip playback.
        let step = rate as f64 / 48_000.0;
        let mut pos = 0.0f64;
        while (pos as usize) + 1 < frames {
            let i = pos as usize;
            let t = (pos - i as f64) as f32;
            for c in 0..2 {
                out.push_back(pick(i, c) * (1.0 - t) + pick(i + 1, c) * t);
            }
            pos += step;
        }
    }

    pub(super) fn run(
        data: Arc<Vec<u8>>,
        shared: Arc<Shared>,
        frames: watch::Sender<Option<Arc<RgbaFrame>>>,
        audio: Arc<Mutex<VecDeque<f32>>>,
        ready: Sender<Result<(), MediaError>>,
    ) {
        // SAFETY: balanced below.
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // SAFETY: balanced with MFShutdown below.
        let mf = unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) };
        let opened = open_reader(&data);
        let (reader, has_video, has_audio) = match opened {
            Ok(r) if r.1 || r.2 => r,
            Ok(_) => {
                let _ = ready.send(Err(MediaError::Audio("no playable video or audio in this file".into())));
                return cleanup(com.is_ok(), mf.is_ok());
            }
            Err(e) => {
                let _ = ready.send(Err(MediaError::Audio(format!("can't play this file: {e}"))));
                return cleanup(com.is_ok(), mf.is_ok());
            }
        };
        let format = read_format(&reader, has_video, has_audio);
        shared.duration_ms.store(duration_ms(&reader), Ordering::Relaxed);
        let _ = ready.send(Ok(()));

        // Playback clock: media time `base_pts` happened at `base_instant`.
        let mut clock: Option<(Instant, i64)> = None;
        let mut paused_at: Option<i64> = None;
        // A file ends once every stream it has reported its end.
        let (mut video_done, mut audio_done) = (!has_video, !has_audio);
        loop {
            if shared.stop.load(Ordering::Relaxed) {
                break;
            }
            if let Some(ms) = shared.seek_to.lock().unwrap_or_else(|e| e.into_inner()).take() {
                seek(&reader, ms);
                audio.lock().unwrap_or_else(|e| e.into_inner()).clear();
                clock = None;
                paused_at = None;
                (video_done, audio_done) = (!has_video, !has_audio);
                shared.position_ms.store(ms, Ordering::Relaxed);
            }
            if !shared.playing.load(Ordering::Relaxed) || shared.ended.load(Ordering::Relaxed) {
                // Freeze the clock while paused.
                if let Some((start, base)) = clock.take() {
                    paused_at = Some(base + start.elapsed().as_millis() as i64 * 10_000);
                }
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }

            let mut stream = 0u32;
            let mut flags = 0u32;
            let mut pts = 0i64;
            let mut sample = None;
            // SAFETY: synchronous read with out parameters.
            let read = unsafe {
                reader.ReadSample(ANY, 0, Some(&mut stream), Some(&mut flags), Some(&mut pts), Some(&mut sample))
            };
            if read.is_err() {
                video_done = true;
                audio_done = true;
            } else if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                if stream_kind(&reader, stream) == Some(MFMediaType_Video) {
                    video_done = true;
                } else {
                    audio_done = true;
                }
            }
            if video_done && audio_done {
                shared.ended.store(true, Ordering::Relaxed);
                shared.playing.store(false, Ordering::Relaxed);
                continue;
            }
            let Some(sample) = sample else { continue };

            let (start, base) = *clock.get_or_insert_with(|| (Instant::now(), paused_at.take().unwrap_or(pts)));
            let is_video = has_video && stream_kind(&reader, stream) == Some(MFMediaType_Video);
            let lead = if is_video { 0 } else { AUDIO_LEAD_MS * 10_000 };
            let due = start + Duration::from_micros(((pts - base - lead).max(0) / 10) as u64);
            while Instant::now() < due {
                if shared.stop.load(Ordering::Relaxed)
                    || shared.seek_to.lock().unwrap_or_else(|e| e.into_inner()).is_some()
                {
                    break;
                }
                std::thread::sleep((due - Instant::now()).min(Duration::from_millis(10)));
            }
            shared.position_ms.store((pts / 10_000).max(0) as u64, Ordering::Relaxed);

            let Some(bytes) = sample_bytes(&sample) else { continue };
            if is_video {
                if let Some(frame) = to_rgba(&bytes, format.width, format.height) {
                    let _ = frames.send(Some(Arc::new(frame)));
                }
            } else {
                let floats: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
                let mut buf = audio.lock().unwrap_or_else(|e| e.into_inner());
                to_stereo_48k(&floats, format.rate, format.channels, &mut buf);
            }
        }
        drop(reader);
        cleanup(com.is_ok(), mf.is_ok());
    }

    fn stream_kind(reader: &IMFSourceReader, stream: u32) -> Option<GUID> {
        // SAFETY: query on an existing stream index.
        unsafe { reader.GetCurrentMediaType(stream).ok()?.GetGUID(&MF_MT_MAJOR_TYPE).ok() }
    }

    fn cleanup(com: bool, mf: bool) {
        // SAFETY: matches the successful startup calls.
        unsafe {
            if mf {
                let _ = MFShutdown();
            }
            if com {
                CoUninitialize();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn converts_bgrx_rows_with_stride() {
            // 2x1 image with a 4-byte row padding.
            let bytes = [1, 2, 3, 0, 4, 5, 6, 0, 9, 9, 9, 9];
            let f = to_rgba(&bytes, 2, 1).unwrap();
            assert_eq!(f.data, [3, 2, 1, 255, 6, 5, 4, 255]);
        }

        #[test]
        fn resamples_mono_to_stereo_48k() {
            let mut out = VecDeque::new();
            to_stereo_48k(&[0.5; 24_000], 24_000, 1, &mut out);
            assert!((out.len() as i64 - 96_000).abs() < 8, "{}", out.len());
            assert!(out.iter().all(|s| (*s - 0.5).abs() < 1e-6));
        }
    }
}
