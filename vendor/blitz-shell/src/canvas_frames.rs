//! Sivryn patch: repaint only when a canvas has something new to show.
//!
//! Upstream treated every document with a `<canvas>` as animating and
//! repainted it at the display's refresh rate for as long as one existed;
//! with Vello repainting the whole window, a single small (even still)
//! image cost 10–20 % of a fast GPU. Canvas sources now call
//! [`request_frame`] (from any thread) when they get a new frame, and the
//! window repaints once.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use winit::event_loop::EventLoopProxy;
use winit::window::WindowId;

use crate::BlitzShellEvent;

static TARGETS: Mutex<Vec<(EventLoopProxy<BlitzShellEvent>, WindowId)>> = Mutex::new(Vec::new());
/// A frame was requested and the windows haven't repainted yet.
static PENDING: AtomicBool = AtomicBool::new(false);

/// Asks every window for one repaint. Requests before that repaint are
/// coalesced into it.
pub fn request_frame() {
    if PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    for (proxy, window_id) in TARGETS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        let _ = proxy.send_event(BlitzShellEvent::Poll { window_id: *window_id });
    }
}

pub(crate) fn register(proxy: EventLoopProxy<BlitzShellEvent>, window_id: WindowId) {
    TARGETS.lock().unwrap_or_else(|e| e.into_inner()).push((proxy, window_id));
}

/// Whether a frame was requested (and resets it).
pub(crate) fn take_pending() -> bool {
    PENDING.swap(false, Ordering::AcqRel)
}
