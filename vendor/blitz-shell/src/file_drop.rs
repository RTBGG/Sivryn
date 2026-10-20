//! Sivryn patch: files dragged onto the window.
//!
//! Blitz ignored winit's file drag-and-drop events. They are now forwarded to
//! a handler the embedding app installs once with [`set_handler`].

use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileDrop {
    /// A file is being dragged over the window.
    Hovered(PathBuf),
    /// A file was dropped onto the window.
    Dropped(PathBuf),
    /// The drag left the window or was cancelled.
    Cancelled,
}

type Handler = Box<dyn Fn(FileDrop) + Send + Sync>;

static HANDLER: OnceLock<Handler> = OnceLock::new();

/// Installs the handler (only the first call takes effect).
pub fn set_handler(handler: impl Fn(FileDrop) + Send + Sync + 'static) {
    let _ = HANDLER.set(Box::new(handler));
}

pub(crate) fn emit(event: FileDrop) {
    if let Some(handler) = HANDLER.get() {
        handler(event);
    }
}
