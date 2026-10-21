//! Sivryn patch: zooming content keeps the point under the mouse in place.
//!
//! A scroll container with a `data-zoom` attribute holds content the app
//! scales (e.g. an image viewer); the value is the current scale. When it
//! changes, the scroll offset is moved so the point of the content under
//! the mouse stays under the mouse (the middle, when the mouse is outside
//! the container), instead of everything growing from the top left corner.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::BaseDocument;

const ATTR: &str = "data-zoom";

/// Per container: the scale and the zoomed content's box (relative to the
/// container, unscrolled) seen after the last layout.
struct Seen {
    scale: String,
    content: (f64, f64, f64, f64),
}

static SEEN: Mutex<Option<HashMap<usize, Seen>>> = Mutex::new(None);
/// The mouse position in window coordinates (set by the shell).
static POINTER: Mutex<Option<(f64, f64)>> = Mutex::new(None);

/// Remembers where the mouse is; zooming keeps the content under it fixed.
pub fn set_pointer(x: f64, y: f64) {
    *POINTER.lock().unwrap_or_else(|e| e.into_inner()) = Some((x, y));
}

/// The box of the container's first element child: what gets zoomed.
fn content_box(doc: &BaseDocument, id: usize) -> Option<(f64, f64, f64, f64)> {
    let child = doc.nodes[id].children.iter().copied().find(|c| doc.nodes[*c].element_data().is_some())?;
    let l = &doc.nodes[child].final_layout;
    Some((l.location.x as f64, l.location.y as f64, l.size.width as f64, l.size.height as f64))
}

/// After layout: move the scroll offset of containers whose scale changed.
pub(crate) fn apply(doc: &mut BaseDocument) {
    let zoomed: Vec<(usize, String)> = doc
        .nodes
        .iter()
        .filter_map(|(id, n)| {
            let el = n.element_data()?;
            el.attrs().iter().find(|a| &*a.name.local == ATTR).map(|a| (id, a.value.clone()))
        })
        .collect();
    let pointer = *POINTER.lock().unwrap_or_else(|e| e.into_inner());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let map = seen.get_or_insert_with(HashMap::new);
    map.retain(|id, _| zoomed.iter().any(|(z, _)| z == id));
    for (id, scale) in zoomed {
        let Some(content) = content_box(doc, id) else { continue };
        if let Some(old) = map.get(&id)
            && old.scale != scale
        {
            let node = &doc.nodes[id];
            let layout = &node.final_layout;
            let (w, h) = (layout.size.width as f64, layout.size.height as f64);
            let max = (layout.scroll_width() as f64, layout.scroll_height() as f64);
            let scroll = (node.scroll_offset.x, node.scroll_offset.y);
            // The anchor within the container: the mouse, or else the middle.
            let origin = node.absolute_position(0.0, 0.0);
            let (ox, oy) = (origin.x as f64 + scroll.0, origin.y as f64 + scroll.1);
            let (ax, ay) = match pointer {
                Some((px, py)) if px >= ox && px <= ox + w && py >= oy && py <= oy + h => (px - ox, py - oy),
                _ => (w / 2.0, h / 2.0),
            };
            // Where the anchor was on the content, as a fraction of its old
            // size, and the scroll offset that puts that spot there again.
            let (x0, y0, w0, h0) = old.content;
            let (x1, y1, w1, h1) = content;
            let fx = if w0 > 0.0 { (scroll.0 + ax - x0) / w0 } else { 0.5 };
            let fy = if h0 > 0.0 { (scroll.1 + ay - y0) / h0 } else { 0.5 };
            let node = &mut doc.nodes[id];
            node.scroll_offset.x = (x1 + fx * w1 - ax).clamp(0.0, max.0.max(0.0));
            node.scroll_offset.y = (y1 + fy * h1 - ay).clamp(0.0, max.1.max(0.0));
        }
        map.insert(id, Seen { scale, content });
    }
}
