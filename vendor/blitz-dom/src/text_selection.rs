//! Sivryn patch: selecting static text with the mouse.
//!
//! Blitz only lets you select text inside text inputs. This adds selection
//! within one inline formatting context (one paragraph, e.g. one chat message)
//! whose element or an ancestor has the class `selectable`: drag to select,
//! double-click selects a word, triple-click a line, Ctrl+C copies. The
//! painter draws the highlight via [`for_node`].

use std::sync::Mutex;
use std::time::{Duration, Instant};

use blitz_traits::events::{BlitzKeyEvent, MouseEventButtons};
use keyboard_types::{Key, Modifiers};
use markup5ever::local_name;
use parley::Selection;

use crate::BaseDocument;

const MULTI_CLICK: Duration = Duration::from_millis(500);

struct State {
    root: usize,
    selection: Selection,
    dragging: bool,
    text: String,
}

struct Click {
    at: Instant,
    root: usize,
    x: f32,
    y: f32,
    count: u8,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static LAST_CLICK: Mutex<Option<Click>> = Mutex::new(None);

fn state() -> std::sync::MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The inline root containing `node`, if it is marked selectable.
fn selectable_root(doc: &BaseDocument, node: usize) -> Option<usize> {
    let mut current = Some(node);
    let mut root = None;
    while let Some(id) = current {
        let n = doc.get_node(id)?;
        if root.is_none() && n.flags.is_inline_root() && n.element_data().is_some_and(|e| e.inline_layout_data.is_some()) {
            root = Some(id);
        }
        if root.is_some()
            && n.element_data()
                .and_then(|e| e.attr(local_name!("class")))
                .is_some_and(|c| c.split_ascii_whitespace().any(|c| c == "selectable"))
        {
            return root;
        }
        current = n.parent;
    }
    None
}

/// Document coordinates to the root's layout coordinates.
fn local_point(doc: &BaseDocument, root: usize, x: f32, y: f32) -> Option<(f32, f32)> {
    let node = doc.get_node(root)?;
    let layout = &node.element_data()?.inline_layout_data.as_ref()?.layout;
    let pos = node.absolute_position(0.0, 0.0);
    let l = &node.final_layout;
    let lx = x - pos.x - l.padding.left - l.border.left;
    let ly = y - pos.y - l.padding.top - l.border.top;
    let scale = layout.scale();
    Some((lx * scale, ly * scale))
}

fn selected_text(doc: &BaseDocument, root: usize, selection: &Selection) -> String {
    let Some(data) = doc.get_node(root).and_then(|n| n.element_data()).and_then(|e| e.inline_layout_data.as_ref()) else {
        return String::new();
    };
    let range = selection.text_range();
    data.text.get(range).unwrap_or_default().to_owned()
}

/// Main-button press. Returns whether a repaint is needed.
pub(crate) fn mouse_down(doc: &BaseDocument, x: f32, y: f32) -> bool {
    let root = doc.hit(x, y).and_then(|hit| selectable_root(doc, hit.node_id));
    let mut state = state();
    let Some(root) = root else {
        return state.take().is_some();
    };
    let Some((lx, ly)) = local_point(doc, root, x, y) else { return false };
    let Some(layout) = doc.get_node(root).and_then(|n| n.element_data()).and_then(|e| e.inline_layout_data.as_ref()).map(|d| &d.layout) else {
        return false;
    };

    let mut last = LAST_CLICK.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let count = match last.as_ref() {
        Some(c) if c.root == root && now - c.at < MULTI_CLICK && (c.x - x).abs() < 4.0 && (c.y - y).abs() < 4.0 => c.count % 3 + 1,
        _ => 1,
    };
    *last = Some(Click { at: now, root, x, y, count });

    let selection = match count {
        2 => Selection::word_from_point(layout, lx, ly),
        3 => Selection::hard_line_from_point(layout, lx, ly),
        _ => Selection::from_point(layout, lx, ly),
    };
    let text = selected_text(doc, root, &selection);
    *state = Some(State { root, selection, dragging: count == 1, text });
    true
}

/// Mouse movement. Returns whether the selection changed.
pub(crate) fn mouse_move(doc: &BaseDocument, x: f32, y: f32, buttons: MouseEventButtons) -> bool {
    let mut state = state();
    let Some(s) = state.as_mut() else { return false };
    if !s.dragging {
        return false;
    }
    if !buttons.contains(MouseEventButtons::Primary) {
        s.dragging = false;
        return false;
    }
    let Some((lx, ly)) = local_point(doc, s.root, x, y) else { return false };
    let Some(layout) = doc.get_node(s.root).and_then(|n| n.element_data()).and_then(|e| e.inline_layout_data.as_ref()).map(|d| &d.layout) else {
        return false;
    };
    let next = s.selection.extend_to_point(layout, lx, ly);
    if next.text_range() == s.selection.text_range() {
        return false;
    }
    s.selection = next;
    s.text = selected_text(doc, s.root, &next);
    true
}

pub(crate) fn mouse_up() {
    if let Some(s) = state().as_mut() {
        s.dragging = false;
    }
}

/// Ctrl+C while no text field has focus copies the selection.
pub(crate) fn handle_copy(doc: &BaseDocument, event: &BlitzKeyEvent) -> bool {
    if !event.state.is_pressed() || !event.modifiers.contains(Modifiers::CONTROL) {
        return false;
    }
    if !matches!(&event.key, Key::Character(c) if c.eq_ignore_ascii_case("c")) {
        return false;
    }
    let in_text_field = doc
        .focus_node_id
        .and_then(|id| doc.get_node(id))
        .and_then(|n| n.element_data())
        .is_some_and(|e| e.text_input_data().is_some());
    if in_text_field {
        return false;
    }
    match selected() {
        Some(text) => {
            let _ = doc.shell_provider.set_clipboard_text(text);
            true
        }
        None => false,
    }
}

/// The selection to paint in this node, if it holds one.
pub fn for_node(node: usize) -> Option<Selection> {
    state().as_ref().filter(|s| s.root == node && !s.selection.is_collapsed()).map(|s| s.selection)
}

/// The selected text, if anything is selected.
pub fn selected() -> Option<String> {
    state().as_ref().filter(|s| !s.selection.is_collapsed() && !s.text.is_empty()).map(|s| s.text.clone())
}

/// Drops the selection (e.g. after copying it from a menu).
pub fn clear() {
    state().take();
}
