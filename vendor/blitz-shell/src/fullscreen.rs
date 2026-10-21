//! Sivryn patch: borderless fullscreen for the app (e.g. a video).
//!
//! The app has no handle to its winit window; [`set_fullscreen`] puts every
//! window into borderless fullscreen on its current monitor, or back, and
//! [`set_cursor_visible`] hides the pointer meanwhile.

use std::sync::{Arc, Mutex};

use winit::window::{Fullscreen, Window};

static WINDOWS: Mutex<Vec<Arc<Window>>> = Mutex::new(Vec::new());

pub(crate) fn register(window: Arc<Window>) {
    WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).push(window);
}

/// Enters or leaves borderless fullscreen (callable from any thread).
pub fn set_fullscreen(on: bool) {
    for window in WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
    }
}

/// Shows or hides the mouse pointer over the windows (e.g. while a
/// fullscreen video's controls are hidden).
pub fn set_cursor_visible(visible: bool) {
    for window in WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        window.set_cursor_visible(visible);
    }
}

/// Whether a window is in fullscreen.
pub fn is_fullscreen() -> bool {
    WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|w| w.fullscreen().is_some())
}
