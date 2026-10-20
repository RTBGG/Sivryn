//! Sivryn patch: a window-wide Escape handler.
//!
//! Key events only reach the focused element, so an overlay without a focused
//! child (an image viewer, a settings page) never saw Escape. The app installs
//! a handler that gets every Escape press first and returns whether it used it.

use std::sync::OnceLock;

type Handler = Box<dyn Fn() -> bool + Send + Sync>;
static HANDLER: OnceLock<Handler> = OnceLock::new();

/// Installs the Escape handler (only the first call takes effect).
pub fn set_escape_handler(handler: impl Fn() -> bool + Send + Sync + 'static) {
    let _ = HANDLER.set(Box::new(handler));
}

pub(crate) fn escape() -> bool {
    HANDLER.get().is_some_and(|h| h())
}
