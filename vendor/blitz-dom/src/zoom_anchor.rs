//! Sivryn patch: zooming content keeps the point in the middle in place.
//!
//! A scroll container with a `data-zoom` attribute holds content the app
//! scales (e.g. an image viewer); the value is the current scale. When it
//! changes, the scroll offset is moved so the point that was in the middle
//! of the container stays there, instead of everything growing from the top
//! left corner.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::BaseDocument;

const ATTR: &str = "data-zoom";

/// Per container: the scale and scrollable range seen after the last layout.
struct Seen {
    scale: String,
    max: (f64, f64),
}

static SEEN: Mutex<Option<HashMap<usize, Seen>>> = Mutex::new(None);

/// After layout: re-centre containers whose scale changed.
pub(crate) fn apply(doc: &mut BaseDocument) {
    let zoomed: Vec<(usize, String)> = doc
        .nodes
        .iter()
        .filter_map(|(id, n)| {
            let el = n.element_data()?;
            el.attrs().iter().find(|a| &*a.name.local == ATTR).map(|a| (id, a.value.clone()))
        })
        .collect();
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let map = seen.get_or_insert_with(HashMap::new);
    map.retain(|id, _| zoomed.iter().any(|(z, _)| z == id));
    for (id, scale) in zoomed {
        let node = &mut doc.nodes[id];
        let layout = &node.final_layout;
        let (w, h) = (layout.size.width as f64, layout.size.height as f64);
        let max = (layout.scroll_width() as f64, layout.scroll_height() as f64);
        if let Some(old) = map.get(&id)
            && old.scale != scale
        {
            // Where the middle was, as a fraction of the old content size;
            // content that fit was centred, so its middle was the middle.
            let fraction = |offset: f64, old_max: f64, size: f64| {
                if old_max <= 0.0 { 0.5 } else { (offset + size / 2.0) / (old_max + size) }
            };
            let fx = fraction(node.scroll_offset.x, old.max.0, w);
            let fy = fraction(node.scroll_offset.y, old.max.1, h);
            node.scroll_offset.x = (fx * (max.0 + w) - w / 2.0).clamp(0.0, max.0);
            node.scroll_offset.y = (fy * (max.1 + h) - h / 2.0).clamp(0.0, max.1);
        }
        map.insert(id, Seen { scale, max });
    }
}
