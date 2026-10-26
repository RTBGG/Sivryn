//! Screen share: capture → I420 → hardware encoder → LiveKit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use livekit::options::{TrackPublishOptions, VideoEncoding};
use livekit::prelude::{LocalTrack, LocalVideoTrack, Room, TrackSource};
use livekit::webrtc::desktop_capturer::{DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions};
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::prelude::{I420Buffer, RtcVideoSource, VideoFrame, VideoResolution, VideoRotation};
use livekit::webrtc::rtp_parameters::DegradationPreference;
use livekit::webrtc::video_source::native::NativeVideoSource;
use sivryn_protocol::media::{ContentHint, ScreenShareLimits, VideoCodec};

use crate::MediaError;
use crate::codec::{self, CodecChoice, HardwareEncoders};

/// A screen or window that can be shared.
#[derive(Clone, Debug)]
pub struct ScreenSource {
    pub id: u64,
    pub title: String,
    pub kind: SourceKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Screen,
    Window,
}

pub(crate) fn capturer(kind: SourceKind) -> Option<DesktopCapturer> {
    let source_type = match kind {
        SourceKind::Screen => DesktopCaptureSourceType::Screen,
        SourceKind::Window => DesktopCaptureSourceType::Window,
    };
    let mut options = DesktopCapturerOptions::new(source_type);
    // The pointer is drawn by `cursor.rs` (libwebrtc's DXGI capturer has none).
    options.set_include_cursor(false);
    DesktopCapturer::new(options)
}

/// Lists screens and windows for the share picker.
pub fn list_sources() -> Vec<ScreenSource> {
    [SourceKind::Screen, SourceKind::Window]
        .into_iter()
        .filter_map(|kind| capturer(kind).map(|c| (kind, c)))
        .flat_map(|(kind, c)| {
            c.get_source_list().into_iter().enumerate().map(move |(i, s)| {
                // Screens usually come without a title.
                let title = match s.title() {
                    t if t.is_empty() && kind == SourceKind::Screen => format!("Screen {}", i + 1),
                    t => t,
                };
                ScreenSource { id: s.id(), title, kind }
            })
        })
        .collect()
}

/// A running screen share. Dropping it stops capture and unpublishes.
pub struct ScreenShare {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    room: Arc<Room>,
    track_sid: livekit::id::TrackSid,
    pub codec: CodecChoice,
    /// A small local preview of what is shared, like a remote feed.
    pub preview: crate::VideoFeed,
}

/// Frame interval of the local screen share preview: the picture as it is
/// sent, in the full stream resolution, at 30 frames per second.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(33);

impl ScreenShare {
    pub async fn start(
        room: Arc<Room>,
        source: &ScreenSource,
        limits: &ScreenShareLimits,
        hint: ContentHint,
        viewers_decode: &[VideoCodec],
    ) -> Result<Self, MediaError> {
        let codec = codec::choose(&limits.codec_preference, HardwareEncoders::detect(), viewers_decode);
        tracing::info!(?codec, title = %source.title, "starting screen share");

        // The real size is only known from the first frame; the source adapts.
        let video_source =
            NativeVideoSource::new(VideoResolution { width: limits.max_width, height: limits.max_height }, true);
        let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(video_source.clone()));

        let options = TrackPublishOptions {
            source: TrackSource::Screenshare,
            video_codec: codec.livekit_codec(),
            video_encoder: codec.encoder_backend(),
            video_encoding: Some(VideoEncoding {
                max_bitrate: limits.max_bitrate_bps,
                max_framerate: limits.max_fps as f64,
            }),
            scalability_mode: codec.scalability_mode(),
            simulcast: codec.simulcast(),
            // WebRTC semantics: MaintainResolution drops frames to keep text
            // sharp, MaintainFramerate lowers resolution to keep motion smooth.
            degradation_preference: Some(match hint {
                ContentHint::Detail => DegradationPreference::MaintainResolution,
                ContentHint::Motion => DegradationPreference::MaintainFramerate,
            }),
            ..Default::default()
        };
        let publication = room.local_participant().publish_track(LocalTrack::Video(track), options).await?;

        let stop = Arc::new(AtomicBool::new(false));
        let (preview_tx, preview_rx) = tokio::sync::watch::channel(None);
        let thread = spawn_capture(source.clone(), limits.clone(), video_source, preview_tx, stop.clone()).await?;
        let preview = crate::VideoFeed {
            participant: room.local_participant().identity().to_string(),
            source: TrackSource::Screenshare,
            track_sid: format!("local-screen-{}", publication.sid()),
            frames: preview_rx,
        };

        Ok(Self { stop, thread: Some(thread), room, track_sid: publication.sid(), codec, preview })
    }

    pub async fn stop(mut self) {
        self.shutdown();
        if let Err(e) = self.room.local_participant().unpublish_track(&self.track_sid).await {
            tracing::warn!(error = %e, "failed to unpublish screen share");
        }
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ScreenShare {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// What happens to every captured picture: scaled to the limits, shown in
/// the local preview now and then, and handed to the encoder.
struct Delivery {
    limits: ScreenShareLimits,
    video_source: NativeVideoSource,
    preview: tokio::sync::watch::Sender<Option<Arc<crate::RgbaFrame>>>,
    started: Instant,
    last_preview: Instant,
}

impl Delivery {
    fn frame(&mut self, mut i420: I420Buffer, w: u32, h: u32) {
        let (tw, th) = fit(w, h, self.limits.max_width, self.limits.max_height);
        let buffer = if (tw, th) != (w, h) { i420.scale(tw as i32, th as i32) } else { i420 };

        // The local preview shows exactly what is sent (libyuv does the
        // scaling and the colour conversion).
        if self.last_preview.elapsed() >= PREVIEW_INTERVAL && self.preview.receiver_count() > 0 {
            self.last_preview = Instant::now();
            let (pw, ph, source) = (tw, th, &buffer);
            let mut data = vec![0u8; (pw * ph * 4) as usize];
            let (y, u, v) = source.data();
            let (sy, su, sv) = source.strides();
            // libyuv's "ABGR" is R, G, B, A in memory.
            yuv_helper::i420_to_abgr(y, sy, u, su, v, sv, &mut data, pw * 4, pw as i32, ph as i32);
            let _ = self.preview.send(Some(Arc::new(crate::RgbaFrame { width: pw, height: ph, data })));
        }

        let mut out = VideoFrame::new(VideoRotation::VideoRotation0, buffer);
        out.timestamp_us = self.started.elapsed().as_micros() as i64;
        self.video_source.capture_frame(&out);
    }
}

/// Runs the capturer on its own thread: platform capturers are not `Send`
/// and expect to be driven from the thread that created them. Sources on an
/// HDR display use Sivryn's own capture with tone mapping instead.
async fn spawn_capture(
    source: ScreenSource,
    limits: ScreenShareLimits,
    video_source: NativeVideoSource,
    preview: tokio::sync::watch::Sender<Option<Arc<crate::RgbaFrame>>>,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, MediaError> {
    // Awaited, not blocked on: callers run on the UI thread.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

    let thread = std::thread::Builder::new()
        .name("sivryn-screen-capture".into())
        .spawn(move || {
            let interval = Duration::from_secs_f64(1.0 / limits.max_fps.max(1) as f64);
            let mut delivery = Delivery {
                limits: limits.clone(),
                video_source,
                preview,
                started: Instant::now(),
                last_preview: Instant::now() - PREVIEW_INTERVAL,
            };
            let mut pointer = crate::cursor::CursorOverlay::new(&source);

            #[cfg(windows)]
            if let Some(hdr_display) = crate::hdr_capture::hdr_display(&source) {
                match crate::hdr_capture::HdrCapture::start(&source, hdr_display) {
                    Ok(mut hdr) => {
                        tracing::info!(
                            sdr_white = hdr_display.sdr_white,
                            peak = hdr_display.peak,
                            "sharing an HDR display, tone-mapped to SDR"
                        );
                        let _ = ready_tx.send(Ok(()));
                        let mut next = Instant::now();
                        let mut last: Option<(Vec<u8>, u32, u32)> = None;
                        while !stop.load(Ordering::Relaxed) {
                            // Without a new picture, the last one goes out again
                            // when only the pointer moved (Windows sends no frame then).
                            let fresh = hdr.next_frame();
                            let is_fresh = fresh.is_some();
                            if is_fresh {
                                last = fresh;
                            }
                            let current = last.as_ref().filter(|(_, w, h)| is_fresh || pointer.moved(*w, *h));
                            if let Some((data, w, h)) = current {
                                let (w, h) = (*w, *h);
                                let (ew, eh) = (w & !1, h & !1);
                                if ew > 0 && eh > 0 {
                                    let mut i420 = I420Buffer::new(ew, eh);
                                    let (sy, su, sv) = i420.strides();
                                    let (y, u, v) = i420.data_mut();
                                    // RGBA, which libyuv calls "ABGR".
                                    yuv_helper::abgr_to_i420(data, w * 4, y, sy, u, su, v, sv, ew as i32, eh as i32);
                                    pointer.draw(&mut i420);
                                    delivery.frame(i420, ew, eh);
                                }
                            }
                            next += interval;
                            match next.checked_duration_since(Instant::now()) {
                                Some(wait) => std::thread::sleep(wait),
                                None => next = Instant::now(),
                            }
                        }
                        return;
                    }
                    Err(e) => tracing::warn!(error = %e, "HDR capture unavailable, sharing without tone mapping"),
                }
            }

            let Some(mut capturer) = capturer(source.kind) else {
                let _ = ready_tx.send(Err(MediaError::CaptureUnavailable));
                return;
            };
            let Some(target) = capturer.get_source_list().into_iter().find(|s| s.id() == source.id) else {
                let _ = ready_tx.send(Err(MediaError::SourceNotFound(source.id)));
                return;
            };

            capturer.start_capture(Some(target), move |result| match result {
                Ok(frame) => {
                    let (w, h) = (frame.width() & !1, frame.height() & !1);
                    if w <= 0 || h <= 0 {
                        return;
                    }
                    // Desktop frames are BGRA, which libyuv calls "ARGB".
                    let mut i420 = I420Buffer::new(w as u32, h as u32);
                    let (sy, su, sv) = i420.strides();
                    let (y, u, v) = i420.data_mut();
                    yuv_helper::argb_to_i420(frame.data(), frame.stride(), y, sy, u, su, v, sv, w, h);
                    pointer.draw(&mut i420);
                    delivery.frame(i420, w as u32, h as u32);
                }
                Err(e) => tracing::trace!(?e, "capture frame failed"),
            });
            let _ = ready_tx.send(Ok(()));

            let mut next = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                capturer.capture_frame();
                next += interval;
                match next.checked_duration_since(Instant::now()) {
                    Some(wait) => std::thread::sleep(wait),
                    // Capture is slower than the target rate; don't try to catch up.
                    None => next = Instant::now(),
                }
            }
        })
        .map_err(|_| MediaError::CaptureUnavailable)?;

    ready_rx.await.map_err(|_| MediaError::CaptureUnavailable)??;
    Ok(thread)
}

/// Largest even size within `max_w × max_h` that keeps the aspect ratio.
fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w <= max_w && h <= max_h {
        return (w, h);
    }
    let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
    let even = |v: f64| ((v as u32) & !1).max(2);
    (even(w as f64 * scale), even(h as f64 * scale))
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn fit_keeps_small_frames() {
        assert_eq!(fit(1920, 1080, 2560, 1440), (1920, 1080));
    }

    #[test]
    fn fit_downscales_4k_to_1440p() {
        assert_eq!(fit(3840, 2160, 2560, 1440), (2560, 1440));
    }

    #[test]
    fn fit_handles_ultrawide() {
        assert_eq!(fit(5120, 1440, 2560, 1440), (2560, 720));
    }
}
