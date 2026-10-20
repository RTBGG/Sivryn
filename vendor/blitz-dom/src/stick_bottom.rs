//! Sivryn patch: scroll containers that stay scrolled to the bottom.
//!
//! An element with a `data-stick-bottom` attribute (e.g. a chat log) starts
//! at the bottom and follows new content there, until the user scrolls up;
//! scrolling back to the bottom pins it again. The attribute's value names
//! the content (e.g. a channel), so switching it starts at the bottom again.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::BaseDocument;

const ATTR: &str = "data-stick-bottom";

/// Containers the user scrolled away from the bottom, by node and value.
static UNPINNED: Mutex<Option<HashMap<usize, String>>> = Mutex::new(None);

fn value(doc: &BaseDocument, node: usize) -> Option<String> {
    let el = doc.get_node(node)?.element_data()?;
    el.attrs().iter().find(|a| &*a.name.local == ATTR).map(|a| a.value.clone())
}

/// After the user scrolled `node`: pinned again only when at the bottom.
pub(crate) fn after_scroll(doc: &BaseDocument, node: usize) {
    let Some(value) = value(doc, node) else { return };
    let Some(n) = doc.get_node(node) else { return };
    let at_bottom = n.scroll_offset.y >= n.final_layout.scroll_height() as f64 - 2.0;
    let mut unpinned = UNPINNED.lock().unwrap_or_else(|e| e.into_inner());
    let map = unpinned.get_or_insert_with(HashMap::new);
    if at_bottom {
        map.remove(&node);
    } else {
        map.insert(node, value);
    }
}

/// After layout: pinned containers jump to their new bottom.
pub(crate) fn apply(doc: &mut BaseDocument) {
    let sticky: Vec<(usize, String)> = doc
        .nodes
        .iter()
        .filter_map(|(id, n)| {
            let el = n.element_data()?;
            el.attrs().iter().find(|a| &*a.name.local == ATTR).map(|a| (id, a.value.clone()))
        })
        .collect();
    let mut unpinned = UNPINNED.lock().unwrap_or_else(|e| e.into_inner());
    let map = unpinned.get_or_insert_with(HashMap::new);
    // Forget containers that are gone or now show something else.
    map.retain(|id, v| sticky.iter().any(|(s, sv)| s == id && sv == v));
    for (id, _) in sticky {
        if map.contains_key(&id) {
            continue;
        }
        let node = &mut doc.nodes[id];
        let bottom = node.final_layout.scroll_height() as f64;
        if node.scroll_offset.y != bottom {
            node.scroll_offset.y = bottom;
        }
    }
}
