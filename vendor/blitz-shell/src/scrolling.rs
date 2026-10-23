//! Sivryn patch: browser-like scrolling.
//!
//! - Mouse wheel: 100 CSS pixels per notch (what Chrome and Edge scroll on
//!   Windows), animated over a short ease-out instead of jumping.
//! - Middle-click autoscroll: click the wheel to start, move the mouse up or
//!   down to scroll (faster the further away), any click ends it. Holding the
//!   wheel and releasing it after moving also ends it, like in browsers.
//! - Ctrl+wheel goes to a handler the app installs ([`set_ctrl_wheel_handler`])
//!   instead of scrolling, e.g. to zoom an image. The plain wheel goes to
//!   [`set_wheel_handler`]'s handler first, which can take it (e.g. to zoom a
//!   screen share under the mouse).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Pixels per wheel notch.
pub const NOTCH: f64 = 100.0;
/// Time constant of the wheel animation (63 % done after this).
const EASE: f64 = 0.045;
/// Mouse distance from the autoscroll origin that doesn't scroll yet.
const DEADZONE: f64 = 12.0;
/// Autoscroll speed in pixels per second per pixel of mouse distance.
const AUTOSCROLL_GAIN: f64 = 9.0;

type CtrlWheel = Box<dyn Fn(f64) -> bool + Send + Sync>;
static CTRL_WHEEL: OnceLock<CtrlWheel> = OnceLock::new();

/// Installs the Ctrl+wheel handler; it gets the notches (positive = up) and
/// returns whether it used them (only the first call takes effect).
pub fn set_ctrl_wheel_handler(handler: impl Fn(f64) -> bool + Send + Sync + 'static) {
    let _ = CTRL_WHEEL.set(Box::new(handler));
}

pub(crate) fn ctrl_wheel(notches: f64) -> bool {
    CTRL_WHEEL.get().is_some_and(|h| h(notches))
}

static WHEEL: OnceLock<CtrlWheel> = OnceLock::new();

/// Installs a handler that sees every plain wheel notch first (positive =
/// up) and returns whether it used it; otherwise the page scrolls.
pub fn set_wheel_handler(handler: impl Fn(f64) -> bool + Send + Sync + 'static) {
    let _ = WHEEL.set(Box::new(handler));
}

pub(crate) fn wheel(notches: f64) -> bool {
    WHEEL.get().is_some_and(|h| h(notches))
}

/// Wheel scrolling still to be applied, per node.
#[derive(Default)]
pub(crate) struct Smooth {
    target: Option<usize>,
    remaining: (f64, f64),
    last: Option<Instant>,
}

impl Smooth {
    pub fn add(&mut self, node: Option<usize>, dx: f64, dy: f64) {
        if self.target != node {
            self.remaining = (0.0, 0.0);
            self.target = node;
        }
        self.remaining.0 += dx;
        self.remaining.1 += dy;
        self.last.get_or_insert_with(Instant::now);
    }

    /// The part to scroll now; `None` when done.
    pub fn step(&mut self) -> Option<(Option<usize>, f64, f64)> {
        let last = self.last?;
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f64().max(0.001);
        self.last = Some(now);
        let k = 1.0 - (-dt / EASE).exp();
        let (rx, ry) = self.remaining;
        let (mut sx, mut sy) = (rx * k, ry * k);
        // Finish the last bit at once instead of crawling.
        if (rx - sx).abs() < 0.5 {
            sx = rx;
        }
        if (ry - sy).abs() < 0.5 {
            sy = ry;
        }
        self.remaining = (rx - sx, ry - sy);
        if self.remaining == (0.0, 0.0) {
            self.last = None;
        }
        Some((self.target, sx, sy))
    }

    pub fn is_active(&self) -> bool {
        self.last.is_some()
    }
}

/// Middle-click autoscroll.
pub(crate) struct Autoscroll {
    pub origin: (f32, f32),
    pub node: Option<usize>,
    pub started: Instant,
    last: Instant,
}

impl Autoscroll {
    pub fn new(origin: (f32, f32), node: Option<usize>) -> Self {
        let now = Instant::now();
        Self { origin, node, started: now, last: now }
    }

    /// Pixels to scroll since the last call for the current mouse position.
    pub fn step(&mut self, mouse: (f32, f32)) -> f64 {
        let now = Instant::now();
        let dt = now.duration_since(self.last).min(Duration::from_millis(100)).as_secs_f64();
        self.last = now;
        let distance = (mouse.1 - self.origin.1) as f64;
        let beyond = distance.abs() - DEADZONE;
        if beyond <= 0.0 {
            return 0.0;
        }
        // Scroll offsets grow downwards; Blitz scrolls by negative deltas.
        -(distance.signum() * beyond * AUTOSCROLL_GAIN * dt)
    }

    /// Releasing the wheel after dragging ends "hold" autoscroll.
    pub fn ends_on_release(&self, mouse: (f32, f32)) -> bool {
        let moved = (mouse.1 - self.origin.1).abs() as f64 > DEADZONE * 2.0;
        moved && self.started.elapsed() > Duration::from_millis(250)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_animation_reaches_the_target() {
        let mut s = Smooth::default();
        s.add(Some(1), 0.0, -NOTCH);
        let mut total = 0.0;
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(2));
            match s.step() {
                Some((_, _, dy)) => total += dy,
                None => break,
            }
        }
        assert!((total + NOTCH).abs() < 1e-6, "scrolled {total}");
        assert!(!s.is_active());
    }

    #[test]
    fn autoscroll_has_a_deadzone_and_speeds_up() {
        let mut a = Autoscroll::new((100.0, 100.0), None);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(a.step((100.0, 105.0)), 0.0);
        std::thread::sleep(Duration::from_millis(20));
        let near = a.step((100.0, 130.0)).abs();
        std::thread::sleep(Duration::from_millis(20));
        let far = a.step((100.0, 300.0)).abs();
        assert!(far > near * 3.0, "{near} vs {far}");
        // Mouse below the origin scrolls down (negative delta in Blitz terms).
        std::thread::sleep(Duration::from_millis(20));
        assert!(a.step((100.0, 200.0)) < 0.0);
    }
}
