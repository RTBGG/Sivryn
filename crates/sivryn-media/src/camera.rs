//! Camera capture via nokhwa (Media Foundation on Windows), published with
//! simulcast so every viewer gets a layer their connection can take.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use livekit::options::{TrackPublishOptions, VideoCodec as LkCodec, VideoEncoding};
use livekit::prelude::{LocalTrack, LocalVideoTrack, Room, TrackSource};
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::prelude::{I420Buffer, RtcVideoSource, VideoBuffer, VideoFrame, VideoResolution, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use nokhwa::Camera;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};
use serde::{Deserialize, Serialize};
use sivryn_protocol::media::{CameraLimits, VideoCodec};

use crate::MediaError;
use crate::codec::HardwareEncoders;

#[cfg(windows)]
const BACKEND: ApiBackend = ApiBackend::MediaFoundation;
#[cfg(not(windows))]
const BACKEND: ApiBackend = ApiBackend::Auto;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraDevice {
    pub index: u32,
    pub name: String,
}

/// Lists cameras. Settings store the name, since indices change when
/// devices are plugged in or out.
pub fn list_cameras() -> Vec<CameraDevice> {
    if !crate::media_foundation_available() {
        return Vec::new();
    }
    match nokhwa::query(BACKEND) {
        Ok(cameras) => cameras
            .into_iter()
            .filter_map(|c| match c.index() {
                CameraIndex::Index(i) => Some(CameraDevice { index: *i, name: c.human_name() }),
                CameraIndex::String(_) => None,
            })
            .collect(),
        Err(e) => {
            tracing::debug!(error = %e, "no cameras");
            Vec::new()
        }
    }
}

/// Camera resolution and frame rate the user prefers (capped by the server).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraQuality {
    P720_30,
    #[default]
    P1080_30,
    P720_60,
    P1080_60,
}

impl CameraQuality {
    pub fn size(self) -> (u32, u32, u32) {
        match self {
            CameraQuality::P720_30 => (1280, 720, 30),
            CameraQuality::P1080_30 => (1920, 1080, 30),
            CameraQuality::P720_60 => (1280, 720, 60),
            CameraQuality::P1080_60 => (1920, 1080, 60),
        }
    }

    /// Bitrate for a clean picture at this size and frame rate (bits/s).
    pub fn bitrate(self) -> u64 {
        match self {
            CameraQuality::P720_30 => 2_500_000,
            CameraQuality::P720_60 => 4_000_000,
            CameraQuality::P1080_30 => 5_000_000,
            CameraQuality::P1080_60 => 8_000_000,
        }
    }
}

/// A published camera. Dropping it stops capture.
pub struct CameraShare {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    room: Arc<Room>,
    track_sid: livekit::id::TrackSid,
    /// Local preview frames (RGBA), like a remote feed.
    pub preview: crate::VideoFeed,
}

impl CameraShare {
    pub async fn start(
        room: Arc<Room>,
        camera_name: Option<&str>,
        quality: CameraQuality,
        limits: &CameraLimits,
    ) -> Result<Self, MediaError> {
        if !crate::media_foundation_available() {
            return Err(MediaError::NoMediaFoundation);
        }
        let cameras = list_cameras();
        let camera = camera_name
            .and_then(|name| cameras.iter().find(|c| c.name == name))
            .or_else(|| cameras.first())
            .cloned()
            .ok_or(MediaError::NoCamera)?;
        let (w, h, fps) = quality.size();
        let (w, h, fps) = (w.min(limits.max_width), h.min(limits.max_height), fps.min(limits.max_fps));

        let video_source = NativeVideoSource::new(VideoResolution { width: w, height: h }, false);
        let (preview_tx, preview_rx) = tokio::sync::watch::channel(None);
        let stop = Arc::new(AtomicBool::new(false));
        let thread = spawn_capture(camera.clone(), (w, h, fps), video_source.clone(), preview_tx, stop.clone()).await?;

        // Hardware H.264 keeps CPU use low; simulcast gives viewers 3 layers.
        let hw = HardwareEncoders::detect();
        let (codec, backend) = if hw.supports(VideoCodec::H264) {
            (LkCodec::H264, livekit::options::VideoEncoderBackend::Hardware)
        } else {
            (LkCodec::VP8, livekit::options::VideoEncoderBackend::default())
        };
        let track = LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(video_source));
        let options = TrackPublishOptions {
            source: TrackSource::Camera,
            video_codec: codec,
            video_encoder: backend,
            video_encoding: Some(VideoEncoding {
                max_bitrate: quality.bitrate().min(limits.max_bitrate_bps),
                max_framerate: fps as f64,
            }),
            simulcast: true,
            ..Default::default()
        };
        let publication = match room.local_participant().publish_track(LocalTrack::Video(track), options).await {
            Ok(p) => p,
            Err(e) => {
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                return Err(e.into());
            }
        };
        let preview = crate::VideoFeed {
            participant: room.local_participant().identity().to_string(),
            source: TrackSource::Camera,
            track_sid: publication.sid().to_string(),
            frames: preview_rx,
        };
        tracing::info!(camera = %camera.name, w, h, fps, ?codec, "camera published");
        Ok(Self { stop, thread: Some(thread), room, track_sid: publication.sid(), preview })
    }

    pub async fn stop(mut self) {
        self.shutdown();
        if let Err(e) = self.room.local_participant().unpublish_track(&self.track_sid).await {
            tracing::warn!(error = %e, "failed to unpublish camera");
        }
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for CameraShare {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Picks the camera mode closest to the target: the largest resolution
/// within it, then the frame rate nearest the target, then the cheapest
/// pixel format to convert (raw YUV before MJPEG).
fn pick_format(formats: &[CameraFormat], (w, h, fps): (u32, u32, u32)) -> Option<CameraFormat> {
    let format_cost = |f: FrameFormat| match f {
        FrameFormat::NV12 => 0,
        FrameFormat::YUYV => 1,
        FrameFormat::MJPEG => 2,
        _ => 9,
    };
    formats
        .iter()
        .filter(|f| format_cost(f.format()) < 9)
        .filter(|f| f.width() <= w && f.height() <= h)
        .max_by_key(|f| {
            let fps_score = -((f.frame_rate() as i64 - fps as i64).abs());
            (f.width() * f.height(), fps_score, -(format_cost(f.format()) as i64))
        })
        .or_else(|| formats.iter().filter(|f| format_cost(f.format()) < 9).min_by_key(|f| f.width() * f.height()))
        .copied()
}

/// Largest size of the local camera preview and its shortest frame interval
/// (at most 60 fps, like the camera).
const PREVIEW_MAX: (u32, u32) = (1280, 720);
const PREVIEW_INTERVAL: std::time::Duration = std::time::Duration::from_millis(15);

async fn spawn_capture(
    camera: CameraDevice,
    target: (u32, u32, u32),
    video_source: NativeVideoSource,
    preview: tokio::sync::watch::Sender<Option<Arc<crate::RgbaFrame>>>,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, MediaError> {
    // Awaited, not blocked on: callers run on the UI thread.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("sivryn-camera".into())
        .spawn(move || {
            let open = || -> Result<Camera, nokhwa::NokhwaError> {
                let any = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
                let mut cam = Camera::with_backend(CameraIndex::Index(camera.index), any, BACKEND)?;
                if let Some(best) = pick_format(&cam.compatible_camera_formats().unwrap_or_default(), target) {
                    cam.set_camera_requset(RequestedFormat::new::<RgbFormat>(RequestedFormatType::Exact(best)))?;
                }
                cam.open_stream()?;
                Ok(cam)
            };
            let mut cam = match open() {
                Ok(cam) => cam,
                Err(e) => {
                    let _ = ready_tx.send(Err(MediaError::Camera(e.to_string())));
                    return;
                }
            };
            tracing::info!(format = %cam.camera_format(), "camera opened");
            let _ = ready_tx.send(Ok(()));

            let started = Instant::now();
            let mut last_preview = Instant::now() - std::time::Duration::from_secs(1);
            while !stop.load(Ordering::Relaxed) {
                let frame = match cam.frame() {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::debug!(error = %e, "camera frame failed");
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    }
                };
                let res = frame.resolution();
                let Some(mut i420) = to_i420(frame.source_frame_format(), frame.buffer(), res.width(), res.height())
                else {
                    continue;
                };
                // The self view at the camera's frame rate (15 fps looked
                // choppy), at most 720p: it is shown small.
                if last_preview.elapsed() >= PREVIEW_INTERVAL && preview.receiver_count() > 0 {
                    last_preview = Instant::now();
                    let (w, h) = (i420.width(), i420.height());
                    let shrink = (PREVIEW_MAX.0 as f64 / w as f64).min(PREVIEW_MAX.1 as f64 / h as f64).min(1.0);
                    let (pw, ph) = (((w as f64 * shrink) as u32) & !1, ((h as f64 * shrink) as u32) & !1);
                    let scaled;
                    let source = if (pw, ph) != (w, h) {
                        scaled = i420.scale(pw as i32, ph as i32);
                        &scaled
                    } else {
                        &i420
                    };
                    let mut rgba = vec![0u8; (pw * ph * 4) as usize];
                    let (sy, su, sv) = source.strides();
                    let (y, u, v) = source.data();
                    yuv_helper::i420_to_abgr(y, sy, u, su, v, sv, &mut rgba, pw * 4, pw as i32, ph as i32);
                    let _ = preview.send(Some(Arc::new(crate::RgbaFrame { width: pw, height: ph, data: rgba })));
                }
                let mut out = VideoFrame::new(VideoRotation::VideoRotation0, i420);
                out.timestamp_us = started.elapsed().as_micros() as i64;
                video_source.capture_frame(&out);
            }
            let _ = cam.stop_stream();
        })
        .map_err(|e| MediaError::Camera(e.to_string()))?;
    ready_rx.await.map_err(|_| MediaError::Camera("camera thread exited".into()))??;
    Ok(thread)
}

/// Converts a camera frame to I420. Odd sizes are cropped by one pixel.
fn to_i420(format: FrameFormat, data: &[u8], width: u32, height: u32) -> Option<I420Buffer> {
    let (w, h) = (width & !1, height & !1);
    if w == 0 || h == 0 {
        return None;
    }
    let mut out = I420Buffer::new(w, h);
    let (sy, su, sv) = out.strides();
    let (y, u, v) = out.data_mut();
    match format {
        FrameFormat::NV12 => {
            let luma = (width * height) as usize;
            if data.len() < luma + luma / 2 {
                return None;
            }
            let (src_y, src_uv) = data.split_at(luma);
            yuv_helper::nv12_to_i420(src_y, width, src_uv, width, y, sy, u, su, v, sv, w as i32, h as i32);
        }
        FrameFormat::YUYV => {
            if data.len() < (width * height * 2) as usize {
                return None;
            }
            yuyv_to_i420(
                data,
                width as usize,
                (w as usize, h as usize),
                (y, sy as usize),
                (u, su as usize),
                (v, sv as usize),
            );
        }
        FrameFormat::MJPEG => {
            let options = zune_jpeg::zune_core::options::DecoderOptions::default()
                .jpeg_set_out_colorspace(zune_jpeg::zune_core::colorspace::ColorSpace::RGBA);
            let mut decoder = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(data), options);
            let rgba = decoder.decode().ok()?;
            let info = decoder.info()?;
            let (jw, jh) = (info.width as u32, info.height as u32);
            if jw < w || jh < h || rgba.len() < (jw * jh * 4) as usize {
                return None;
            }
            // libyuv's "ABGR" is R, G, B, A in memory.
            yuv_helper::abgr_to_i420(&rgba, jw * 4, y, sy, u, su, v, sv, w as i32, h as i32);
        }
        _ => return None,
    }
    Some(out)
}

/// YUY2 (Y0 U Y1 V per pixel pair) to I420, averaging chroma over two rows.
fn yuyv_to_i420(
    src: &[u8],
    src_width: usize,
    (w, h): (usize, usize),
    (y, sy): (&mut [u8], usize),
    (u, su): (&mut [u8], usize),
    (v, sv): (&mut [u8], usize),
) {
    let stride = src_width * 2;
    for row in 0..h {
        let line = &src[row * stride..row * stride + w * 2];
        for col in 0..w {
            y[row * sy + col] = line[col * 2];
        }
    }
    for row in 0..h / 2 {
        let a = &src[(row * 2) * stride..];
        let b = &src[(row * 2 + 1) * stride..];
        for col in 0..w / 2 {
            let i = col * 4;
            u[row * su + col] = (a[i + 1] as u16 + b[i + 1] as u16).div_ceil(2) as u8;
            v[row * sv + col] = (a[i + 3] as u16 + b[i + 3] as u16).div_ceil(2) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(w: u32, h: u32, f: FrameFormat, fps: u32) -> CameraFormat {
        CameraFormat::new_from(w, h, f, fps)
    }

    #[test]
    fn picks_largest_fitting_mode_with_cheapest_format() {
        let formats = [
            fmt(640, 480, FrameFormat::YUYV, 30),
            fmt(1920, 1080, FrameFormat::MJPEG, 30),
            fmt(1920, 1080, FrameFormat::YUYV, 5),
            fmt(1280, 720, FrameFormat::NV12, 30),
            fmt(3840, 2160, FrameFormat::MJPEG, 30),
        ];
        assert_eq!(pick_format(&formats, (1920, 1080, 30)), Some(fmt(1920, 1080, FrameFormat::MJPEG, 30)));
        assert_eq!(pick_format(&formats, (1280, 720, 30)), Some(fmt(1280, 720, FrameFormat::NV12, 30)));
        // Nothing small enough: take the smallest mode instead of failing.
        assert_eq!(pick_format(&formats[1..2], (640, 360, 30)), Some(fmt(1920, 1080, FrameFormat::MJPEG, 30)));
    }

    #[test]
    fn yuyv_converts_to_planar() {
        // 2x2 image: Y values 10, 20 / 30, 40; U 100/120, V 200/220 per row.
        let src = [10, 100, 20, 200, 30, 120, 40, 220];
        let (mut y, mut u, mut v) = ([0u8; 4], [0u8; 1], [0u8; 1]);
        yuyv_to_i420(&src, 2, (2, 2), (&mut y, 2), (&mut u, 1), (&mut v, 1));
        assert_eq!(y, [10, 20, 30, 40]);
        assert_eq!(u, [110]);
        assert_eq!(v, [210]);
    }
}
