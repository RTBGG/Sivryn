//! Sivryn patch: text caret blinking.
//!
//! Blitz drew the caret permanently. It now blinks at the Windows default
//! rate and restarts visible after every key press, click or focus change.
//! Blitz only repaints when something changes, so the embedding app calls
//! [`ms_until_toggle`] to schedule a repaint at each toggle while
//! [`input_focused`] is true (the painter reports drawing a caret via
//! [`mark_drawn`]).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

/// Half a blink cycle, in milliseconds (Windows `GetCaretBlinkTime` default).
pub const BLINK_MS: u64 = 530;

static LAST_RESET: AtomicU64 = AtomicU64::new(0);
static LAST_DRAWN: AtomicU64 = AtomicU64::new(0);
static EVER_DRAWN: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Shows the caret now and restarts the blink cycle.
pub fn reset() {
    LAST_RESET.store(now_ms(), Ordering::Relaxed);
}

pub fn visible() -> bool {
    let elapsed = now_ms().saturating_sub(LAST_RESET.load(Ordering::Relaxed));
    (elapsed / BLINK_MS) % 2 == 0
}

/// Time until the caret next appears or disappears.
pub fn ms_until_toggle() -> u64 {
    let elapsed = now_ms().saturating_sub(LAST_RESET.load(Ordering::Relaxed));
    BLINK_MS - elapsed % BLINK_MS
}

/// Called by the painter whenever it paints a focused text field.
pub fn mark_drawn() {
    LAST_DRAWN.store(now_ms(), Ordering::Relaxed);
    EVER_DRAWN.store(true, Ordering::Relaxed);
}

/// Whether a focused text field was painted recently, i.e. a blinking caret
/// is on screen and needs repaints.
pub fn input_focused() -> bool {
    EVER_DRAWN.load(Ordering::Relaxed) && now_ms().saturating_sub(LAST_DRAWN.load(Ordering::Relaxed)) < 2 * BLINK_MS + 200
}

pub(crate) fn set_input_focused(_focused: bool) {
    reset();
}
