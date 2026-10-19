//! Screen share audio via Windows process loopback (Windows 10 2004+).
//!
//! - Sharing a window captures only that application's audio (its process
//!   tree, so browsers' audio subprocesses are included).
//! - Sharing a screen captures all system audio *except Sivryn itself*, so
//!   viewers never hear their own voices echoed back.
//!
//! Audio is published as LiveKit's `ScreenshareAudio` source: Opus stereo at
//! 128 kbit/s without DTX, tuned for music and game sound rather than voice.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::{LocalAudioTrack, LocalTrack, Room, TrackSource};
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};

use crate::MediaError;
use crate::audio::dsp::{FRAME, RATE};

/// Whose audio to capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopbackTarget {
    /// This process and its children (a shared window's application).
    Process(u32),
    /// Everything except this process tree (Sivryn itself).
    AllExcept(u32),
}

/// Process that owns a shared window, for [`LoopbackTarget::Process`].
/// Screen and window source IDs from the desktop capturer are HWNDs on Windows.
pub fn window_process(source_id: u64) -> Option<u32> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
        let mut pid = 0u32;
        // SAFETY: GetWindowThreadProcessId only reads the handle; an invalid one yields 0.
        unsafe { GetWindowThreadProcessId(HWND(source_id as usize as *mut _), Some(&mut pid)) };
        (pid != 0).then_some(pid)
    }
    #[cfg(not(windows))]
    {
        let _ = source_id;
        None
    }
}

/// A running screen audio capture. Dropping it stops capture.
pub struct ScreenAudio {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    room: Arc<Room>,
    track_sid: livekit::id::TrackSid,
}

impl ScreenAudio {
    pub async fn start(room: Arc<Room>, target: LoopbackTarget) -> Result<Self, MediaError> {
        let source = NativeAudioSource::new(AudioSourceOptions::default(), RATE, 2, 0);
        let stop = Arc::new(AtomicBool::new(false));
        let thread = spawn_loopback(target, source.clone(), stop.clone()).await?;

        let track = LocalAudioTrack::create_audio_track("screen-audio", RtcAudioSource::Native(source));
        let options = TrackPublishOptions {
            source: TrackSource::ScreenshareAudio,
            audio_encoding: Some(AudioEncoding { max_bitrate: 128_000 }),
            // Music and games have quiet passages that DTX would chop up.
            dtx: false,
            red: true,
            ..Default::default()
        };
        let publication = match room.local_participant().publish_track(LocalTrack::Audio(track), options).await {
            Ok(p) => p,
            Err(e) => {
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                return Err(e.into());
            }
        };
        Ok(Self { stop, thread: Some(thread), room, track_sid: publication.sid() })
    }

    pub async fn stop(mut self) {
        self.shutdown();
        if let Err(e) = self.room.local_participant().unpublish_track(&self.track_sid).await {
            tracing::warn!(error = %e, "failed to unpublish screen audio");
        }
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ScreenAudio {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Captured samples while [`capture_probe`] runs (diagnostics only).
static PROBE: std::sync::Mutex<Option<Vec<i16>>> = std::sync::Mutex::new(None);

/// Diagnostics: captures for `seconds` without LiveKit and returns the
/// interleaved stereo samples (used by `examples/loopback.rs`).
#[doc(hidden)]
pub fn capture_probe(target: LoopbackTarget, seconds: u64) -> Result<Vec<i16>, MediaError> {
    *PROBE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Vec::new());
    let source = NativeAudioSource::new(AudioSourceOptions::default(), RATE, 2, 0);
    let stop = Arc::new(AtomicBool::new(false));
    let thread = futures::executor::block_on(spawn_loopback(target, source, stop.clone()))?;
    std::thread::sleep(std::time::Duration::from_secs(seconds));
    stop.store(true, Ordering::Relaxed);
    let _ = thread.join();
    Ok(PROBE.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default())
}

async fn spawn_loopback(
    target: LoopbackTarget,
    source: NativeAudioSource,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, MediaError> {
    // Awaited, not blocked on: callers run on the UI thread.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("sivryn-screen-audio".into())
        .spawn(move || {
            #[cfg(windows)]
            wasapi::run(target, source, stop, ready_tx);
            #[cfg(not(windows))]
            {
                let _ = (target, source, stop);
                let _ = ready_tx.send(Err(MediaError::Audio("screen audio is only supported on Windows".into())));
            }
        })
        .map_err(|e| MediaError::Audio(e.to_string()))?;
    match ready_rx.await {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err(MediaError::Audio("screen audio thread exited".into())),
    }
}

/// Splits interleaved stereo i16 into 10 ms frames and hands them to LiveKit.
struct Framer {
    pending: Vec<i16>,
    source: NativeAudioSource,
}

impl Framer {
    fn push(&mut self, samples: &[i16]) {
        if let Some(probe) = PROBE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            probe.extend_from_slice(samples);
        }
        self.pending.extend_from_slice(samples);
        let frame_len = FRAME * 2;
        let mut offset = 0;
        while self.pending.len() - offset >= frame_len {
            let data = self.pending[offset..offset + frame_len].to_vec();
            offset += frame_len;
            let frame =
                AudioFrame { data: data.into(), sample_rate: RATE, num_channels: 2, samples_per_channel: FRAME as u32 };
            if let Err(e) = futures::executor::block_on(self.source.capture_frame(&frame)) {
                tracing::trace!(error = %e, "dropped screen audio frame");
            }
        }
        self.pending.drain(..offset);
    }
}

#[cfg(windows)]
mod wasapi {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::StructuredStorage::{
        PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
    };
    use windows::Win32::System::Com::{
        BLOB, COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemAlloc, CoUninitialize, IAgileObject, IAgileObject_Impl,
    };
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
    use windows::Win32::System::Variant::VT_BLOB;
    use windows::core::{Interface, Ref, implement};

    use super::{Framer, LoopbackTarget, NativeAudioSource};
    use crate::MediaError;
    use crate::audio::dsp::RATE;

    /// Signals an event when `ActivateAudioInterfaceAsync` has finished.
    // Activation calls back on another thread; the handler must be agile.
    #[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
    struct Completion(HANDLE);

    impl IAgileObject_Impl for Completion_Impl {}

    impl IActivateAudioInterfaceCompletionHandler_Impl for Completion_Impl {
        fn ActivateCompleted(
            &self,
            _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
        ) -> windows::core::Result<()> {
            // SAFETY: the event outlives the activation (activate() waits on it).
            unsafe { SetEvent(self.0) }
        }
    }

    struct Event(HANDLE);
    impl Drop for Event {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateEventW and is closed once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn activate(target: LoopbackTarget) -> windows::core::Result<IAudioClient> {
        let (pid, mode) = match target {
            LoopbackTarget::Process(pid) => (pid, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE),
            LoopbackTarget::AllExcept(pid) => (pid, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE),
        };
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: mode,
                },
            },
        };
        // The blob must be COM-allocated: activation releases the PROPVARIANT
        // with PropVariantClear, which frees the blob data. A blob on the
        // stack corrupted the heap.
        // SAFETY: COM allocation of the right size, initialized right away.
        let params_ptr = unsafe {
            let p = CoTaskMemAlloc(size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>()) as *mut AUDIOCLIENT_ACTIVATION_PARAMS;
            p.write(params);
            p
        };
        let blob = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                            pBlobData: params_ptr.cast(),
                        },
                    },
                }),
            },
        };
        // SAFETY: plain event creation; closed by `Event`'s Drop.
        let done = Event(unsafe { CreateEventW(None, true, false, None)? });
        let handler: IActivateAudioInterfaceCompletionHandler = Completion(done.0).into();
        // SAFETY: `blob` outlives the call; its data is COM-allocated (see above).
        let operation = unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&blob),
                &handler,
            )?
        };
        // SAFETY: waiting on our own event.
        if unsafe { WaitForSingleObject(done.0, 5000) } != WAIT_OBJECT_0 {
            return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL));
        }
        let mut hr = windows::core::HRESULT(0);
        let mut unknown = None;
        // SAFETY: the operation has completed (signalled above).
        unsafe { operation.GetActivateResult(&mut hr, &mut unknown)? };
        hr.ok()?;
        unknown.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?.cast()
    }

    pub(super) fn run(
        target: LoopbackTarget,
        source: NativeAudioSource,
        stop: Arc<AtomicBool>,
        ready: tokio::sync::oneshot::Sender<Result<(), MediaError>>,
    ) {
        // SAFETY: balanced by CoUninitialize at the end of this thread.
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let result = (|| -> windows::core::Result<(IAudioClient, IAudioCaptureClient, Event)> {
            let client = activate(target)?;
            // Process loopback converts to whatever format we ask for.
            let format = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_PCM as u16,
                nChannels: 2,
                nSamplesPerSec: RATE,
                nAvgBytesPerSec: RATE * 4,
                nBlockAlign: 4,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            // SAFETY: plain COM calls on a freshly activated client.
            unsafe {
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK
                        | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                    200_000, // 20 ms in 100 ns units
                    0,
                    &format,
                    None,
                )?;
                let event = Event(CreateEventW(None, false, false, None)?);
                client.SetEventHandle(event.0)?;
                let capture: IAudioCaptureClient = client.GetService()?;
                client.Start()?;
                Ok((client, capture, event))
            }
        })();

        let (client, capture, event) = match result {
            Ok(parts) => {
                let _ = ready.send(Ok(()));
                parts
            }
            Err(e) => {
                tracing::warn!(error = %e, ?target, "screen audio capture unavailable");
                let _ = ready.send(Err(MediaError::Audio(format!("screen audio unavailable: {e}"))));
                if com.is_ok() {
                    // SAFETY: matches the successful CoInitializeEx above.
                    unsafe { CoUninitialize() };
                }
                return;
            }
        };
        tracing::info!(?target, "screen audio capture started");

        let mut framer = Framer { pending: Vec::new(), source };
        while !stop.load(Ordering::Relaxed) {
            // SAFETY: waiting on our own event handle.
            if unsafe { WaitForSingleObject(event.0, 100) } != WAIT_OBJECT_0 {
                continue;
            }
            loop {
                // SAFETY: standard WASAPI capture loop; each GetBuffer is
                // paired with ReleaseBuffer and the data copied in between.
                let packet = unsafe { capture.GetNextPacketSize() }.unwrap_or(0);
                if packet == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                if unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.is_err() {
                    break;
                }
                let count = frames as usize * 2;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    framer.push(&vec![0i16; count]);
                } else {
                    let samples = unsafe { std::slice::from_raw_parts(data as *const i16, count) };
                    framer.push(samples);
                }
                let _ = unsafe { capture.ReleaseBuffer(frames) };
            }
        }

        // SAFETY: stopping the client we started; COM is uninitialized last.
        unsafe {
            let _ = client.Stop();
        }
        drop((capture, client, event));
        if com.is_ok() {
            unsafe { CoUninitialize() };
        }
    }
}
