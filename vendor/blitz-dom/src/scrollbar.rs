//! Sivryn patch: vertical scroll bars.
//!
//! Blitz scrolls but draws no scroll bars. Every element with
//! `overflow-y: auto | scroll` whose content is taller than its box gets a
//! thin bar over its right edge (it takes no layout space). blitz-paint
//! draws it from [`vertical`]; the shell feeds mouse input to
//! [`BaseDocument::scrollbar_press`] and friends, so the thumb can be dragged
//! and a click in the track jumps there.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use style::values::computed::Overflow;

use crate::{BaseDocument, Node};

/// Width of the strip at the right edge that grabs the bar.
const GRAB_WIDTH: f32 = 12.0;
/// Width of the drawn thumb.
pub const THUMB_WIDTH: f32 = 6.0;
/// Gap between the thumb and the box's edges.
const INSET: f32 = 2.0;
const MIN_THUMB: f32 = 24.0;

/// A scroll bar in the node's own coordinates (CSS pixels from its border box).
#[derive(Clone, Copy, Debug)]
pub struct Bar {
    pub left: f32,
    pub track_top: f32,
    pub track_len: f32,
    pub thumb_top: f32,
    pub thumb_len: f32,
    pub max_scroll: f64,
}

/// The node's vertical scroll bar, if it has one.
pub fn vertical(node: &Node) -> Option<Bar> {
    let styles = node.primary_styles()?;
    if !matches!(styles.clone_overflow_y(), Overflow::Auto | Overflow::Scroll) {
        return None;
    }
    let max_scroll = node.final_layout.scroll_height() as f64;
    if max_scroll < 1.0 {
        return None;
    }
    let layout = &node.final_layout;
    let inner = layout.size.height - layout.border.top - layout.border.bottom;
    let track_top = layout.border.top + INSET;
    let track_len = inner - 2.0 * INSET;
    if track_len < MIN_THUMB * 1.5 {
        return None;
    }
    let thumb_len = (track_len * inner / (inner + max_scroll as f32)).max(MIN_THUMB);
    let progress = (node.scroll_offset.y / max_scroll).clamp(0.0, 1.0) as f32;
    Some(Bar {
        left: layout.size.width - layout.border.right - INSET - THUMB_WIDTH,
        track_top,
        track_len,
        thumb_top: track_top + (track_len - thumb_len) * progress,
        thumb_len,
        max_scroll,
    })
}

const NONE: usize = usize::MAX;
/// The node whose bar the pointer is over (drawn brighter).
static HOVERED: AtomicUsize = AtomicUsize::new(NONE);

/// The bar being dragged: its node and where on the thumb it was grabbed.
static DRAG: Mutex<Option<(usize, f32)>> = Mutex::new(None);

fn dragged() -> Option<(usize, f32)> {
    *DRAG.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the node's bar is hovered or dragged.
pub fn is_highlighted(node_id: usize) -> bool {
    HOVERED.load(Ordering::Relaxed) == node_id || dragged().is_some_and(|(n, _)| n == node_id)
}

impl BaseDocument {
    /// The top left corner of the node's border box in window coordinates.
    fn border_box_origin(&self, node: &Node) -> (f32, f32) {
        let p = node.absolute_position(0.0, 0.0);
        (p.x + node.scroll_offset.x as f32, p.y + node.scroll_offset.y as f32)
    }

    /// The scroll bar under the point: its node, the bar and the point's
    /// height within the node.
    fn scrollbar_at(&self, x: f32, y: f32) -> Option<(usize, Bar, f32)> {
        let mut id = Some(self.hit(x, y)?.node_id);
        while let Some(node_id) = id {
            let node = &self.nodes[node_id];
            if let Some(bar) = vertical(node) {
                let (ox, oy) = self.border_box_origin(node);
                let (lx, ly) = (x - ox, y - oy);
                let layout = &node.final_layout;
                let right = layout.size.width - layout.border.right;
                if lx >= right - GRAB_WIDTH
                    && lx <= right
                    && ly >= layout.border.top
                    && ly <= layout.size.height - layout.border.bottom
                {
                    return Some((node_id, bar, ly));
                }
            }
            id = node.layout_parent.get();
        }
        None
    }

    fn drag_to(&mut self, node_id: usize, grab: f32, y: f32) {
        let Some(node) = self.nodes.get(node_id) else { return };
        let Some(bar) = vertical(node) else { return };
        let (_, oy) = self.border_box_origin(node);
        let room = (bar.track_len - bar.thumb_len).max(1.0);
        let progress = ((y - oy - grab - bar.track_top) / room).clamp(0.0, 1.0) as f64;
        self.nodes[node_id].scroll_offset.y = progress * bar.max_scroll;
        crate::stick_bottom::after_scroll(self, node_id);
    }

    /// Left button pressed: grabs the thumb, or jumps there for a click in
    /// the track. Returns whether the press belonged to a scroll bar.
    pub fn scrollbar_press(&mut self, x: f32, y: f32) -> bool {
        let Some((node_id, bar, ly)) = self.scrollbar_at(x, y) else { return false };
        let on_thumb = ly >= bar.thumb_top && ly <= bar.thumb_top + bar.thumb_len;
        let grab = if on_thumb { ly - bar.thumb_top } else { bar.thumb_len / 2.0 };
        *DRAG.lock().unwrap_or_else(|e| e.into_inner()) = Some((node_id, grab));
        if !on_thumb {
            self.drag_to(node_id, grab, y);
        }
        true
    }

    /// Pointer moved. Returns `(consumed, repaint)`: consumed while a thumb
    /// is dragged (the page shouldn't see the move), repaint when something
    /// changed.
    pub fn scrollbar_move(&mut self, x: f32, y: f32) -> (bool, bool) {
        if let Some((node_id, grab)) = dragged() {
            self.drag_to(node_id, grab, y);
            return (true, true);
        }
        let hovered = self.scrollbar_at(x, y).map_or(NONE, |(id, _, _)| id);
        let before = HOVERED.swap(hovered, Ordering::Relaxed);
        (false, before != hovered)
    }

    /// Left button released. Returns whether it ended a drag.
    pub fn scrollbar_release(&mut self) -> bool {
        DRAG.lock().unwrap_or_else(|e| e.into_inner()).take().is_some()
    }
}
