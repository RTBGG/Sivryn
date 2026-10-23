//! Sivryn patch: the app controlling its window.
//!
//! - [`set_visible`] and [`show_and_focus`] hide the window to the
//!   notification area and bring it back.
//! - [`set_close_handler`]: the close button asks the app first, which can
//!   hide the window instead of quitting.
//! - [`native_handles`] gives the platform window handles (e.g. for taskbar
//!   buttons on Windows).

use std::sync::{Arc, Mutex, OnceLock};

use winit::window::Window;

static WINDOWS: Mutex<Vec<Arc<Window>>> = Mutex::new(Vec::new());

type CloseHandler = Box<dyn Fn() -> bool + Send + Sync>;
static CLOSE: OnceLock<CloseHandler> = OnceLock::new();

pub(crate) fn register(window: Arc<Window>) {
    WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).push(window);
}

pub(crate) fn windows() -> Vec<Arc<Window>> {
    WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Installs the close handler: it returns `true` to keep running (the
/// window is hidden instead of closed). Only the first call takes effect.
pub fn set_close_handler(handler: impl Fn() -> bool + Send + Sync + 'static) {
    let _ = CLOSE.set(Box::new(handler));
}

/// Whether the close button should hide the window instead of quitting.
pub(crate) fn hide_on_close() -> bool {
    CLOSE.get().is_some_and(|h| h())
}

/// Shows or hides the windows (hidden ones have no taskbar button).
pub fn set_visible(visible: bool) {
    for window in windows() {
        window.set_visible(visible);
    }
}

/// Shows the windows, restores them when minimized and gives them focus.
pub fn show_and_focus() {
    for window in windows() {
        window.set_visible(true);
        window.set_minimized(false);
        window.focus_window();
    }
}

/// Whether a window is shown.
pub fn is_visible() -> bool {
    windows().iter().any(|w| w.is_visible().unwrap_or(true))
}

/// The platform handles of the windows (HWNDs on Windows).
pub fn native_handles() -> Vec<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    windows()
        .iter()
        .filter_map(|w| match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        })
        .collect()
}
