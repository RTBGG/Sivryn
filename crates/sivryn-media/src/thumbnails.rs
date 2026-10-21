//! Live thumbnails of screens and windows for the share picker.
//!
//! Uses the same platform capturer as screen sharing (Windows Graphics
//! Capture on Windows), so games and GPU-accelerated windows show up as they
//! will be shared. Each source is captured once per second, scaled down on
//! the capture thread, while the [`SourcePreviews`] value is alive.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use tokio::sync::watch;

use crate::RgbaFrame;
use crate::screen::{ScreenSource, capturer};

/// Longest edge of a thumbnail, in pixels.
const THUMB_EDGE: u32 = 400;
const REFRESH: Duration = Duration::from_secs(1);

pub type Thumbnail = watch::Receiver<Option<Arc<RgbaFrame>>>;

/// Captures thumbnails until dropped.
pub struct SourcePreviews {
    pub frames: HashMap<u64, Thumbnail>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SourcePreviews {
    pub fn start(sources: &[ScreenSource]) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let mut frames = HashMap::new();
        let mut senders = Vec::new();
        for source in sources {
            let (tx, rx) = watch::channel(None);
            frames.insert(source.id, rx);
            senders.push((source.clone(), tx));
        }
        let flag = stop.clone();
        let thread =
            std::thread::Builder::new().name("sivryn-share-previews".into()).spawn(move || run(senders, flag)).ok();
        Self { frames, stop, thread }
    }
}

impl Drop for SourcePreviews {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Capturers are not `Send`, so they all live on this thread.
fn run(sources: Vec<(ScreenSource, watch::Sender<Option<Arc<RgbaFrame>>>)>, stop: Arc<AtomicBool>) {
    let mut capturers = Vec::new();
    for (source, tx) in sources {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let Some(mut capturer) = capturer(source.kind) else { continue };
        let Some(target) = capturer.get_source_list().into_iter().find(|s| s.id() == source.id) else { continue };
        capturer.start_capture(Some(target), move |result| {
            if let Ok(frame) = result
                && let Some(thumb) =
                    downscale(frame.data(), frame.stride() as usize, frame.width(), frame.height(), THUMB_EDGE)
            {
                let _ = tx.send(Some(Arc::new(thumb)));
            }
        });
        // The first frame right away, so the picker fills in quickly.
        capturer.capture_frame();
        capturers.push(capturer);
    }
    while !stop.load(Ordering::Relaxed) {
        // Sleep in small steps so closing the picker never waits long.
        for _ in 0..10 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(REFRESH / 10);
        }
        for capturer in &mut capturers {
            capturer.capture_frame();
        }
    }
}

/// Box-filters a BGRA desktop frame down to at most `edge` pixels on its
/// longest edge, as RGBA.
pub(crate) fn downscale(data: &[u8], stride: usize, width: i32, height: i32, edge: u32) -> Option<RgbaFrame> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let (w, h) = (width as u32, height as u32);
    let scale = (edge as f64 / w.max(h) as f64).min(1.0);
    let (tw, th) = (((w as f64 * scale) as u32).max(1), ((h as f64 * scale) as u32).max(1));
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for ty in 0..th {
        let y0 = (ty * h / th) as usize;
        let y1 = (((ty + 1) * h / th) as usize).max(y0 + 1).min(h as usize);
        for tx in 0..tw {
            let x0 = (tx * w / tw) as usize;
            let x1 = (((tx + 1) * w / tw) as usize).max(x0 + 1).min(w as usize);
            let (mut b, mut g, mut r, mut n) = (0u32, 0u32, 0u32, 0u32);
            // Sample at most a 4×4 grid per output pixel; plenty for a thumbnail.
            let sy = ((y1 - y0) / 4).max(1);
            let sx = ((x1 - x0) / 4).max(1);
            for y in (y0..y1).step_by(sy) {
                let row = y * stride;
                for x in (x0..x1).step_by(sx) {
                    let i = row + x * 4;
                    let Some(px) = data.get(i..i + 4) else { continue };
                    b += px[0] as u32;
                    g += px[1] as u32;
                    r += px[2] as u32;
                    n += 1;
                }
            }
            let n = n.max(1);
            let o = ((ty * tw + tx) * 4) as usize;
            out[o..o + 4].copy_from_slice(&[(r / n) as u8, (g / n) as u8, (b / n) as u8, 255]);
        }
    }
    Some(RgbaFrame { width: tw, height: th, data: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_keep_aspect_and_convert_colors() {
        // 800×400 BGRA, pure blue.
        let (w, h) = (800, 400);
        let data: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
        let thumb = downscale(&data, w * 4, w as i32, h as i32, THUMB_EDGE).unwrap();
        assert_eq!((thumb.width, thumb.height), (400, 200));
        assert_eq!(&thumb.data[..4], &[0, 0, 255, 255]);
        assert!(downscale(&[], 0, 0, 0, THUMB_EDGE).is_none());
        // Small sources aren't enlarged.
        let tiny: Vec<u8> = vec![0; 10 * 10 * 4];
        assert_eq!(downscale(&tiny, 40, 10, 10, THUMB_EDGE).unwrap().width, 10);
    }
}
