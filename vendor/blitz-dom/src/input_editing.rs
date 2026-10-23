//! Sivryn patch: selecting and editing in text inputs with the mouse.
//!
//! - Double-click selects a word, triple-click everything (inputs are one
//!   line).
//! - A drag that starts in an input keeps selecting there when the pointer
//!   leaves it, and releasing it over another element doesn't click that
//!   element (dragging over a chat's "+" button opened the file picker).
//! - A right click notes the input under the pointer and whether it has a
//!   selection ([`context`]), so the app can offer Copy, Cut and Paste;
//!   [`request`] runs one of those (the shell carries it out as the matching
//!   Ctrl key, so the app sees the usual input event).
//! - [`request_focus`] puts the keyboard focus (and the caret) into an input
//!   by its `id`, e.g. back into a chat's message box after picking an emoji.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::BaseDocument;

const MULTI_CLICK: Duration = Duration::from_millis(500);

struct Click {
    at: Instant,
    node: usize,
    x: f32,
    y: f32,
    count: u8,
}

static LAST_CLICK: Mutex<Option<Click>> = Mutex::new(None);
/// The input a drag selection started in.
static DRAG: Mutex<Option<usize>> = Mutex::new(None);
/// The input under the last right click and whether it had a selection.
static CONTEXT: Mutex<Option<(usize, bool)>> = Mutex::new(None);
/// An edit the app asked for, on that input.
static PENDING: Mutex<Option<(usize, Edit)>> = Mutex::new(None);
/// The `id` of an element the app wants focused.
static FOCUS: Mutex<Option<String>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// An edit from a context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Copy,
    Cut,
    Paste,
    SelectAll,
}

impl Edit {
    /// The Ctrl key that does the same.
    pub fn key(self) -> &'static str {
        match self {
            Edit::Copy => "c",
            Edit::Cut => "x",
            Edit::Paste => "v",
            Edit::SelectAll => "a",
        }
    }
}

/// A mouse press in a text input: 1 for a single click, 2 for a double
/// click, 3 for a triple click (and more).
pub(crate) fn press(node: usize, x: f32, y: f32) -> u8 {
    let now = Instant::now();
    let mut last = lock(&LAST_CLICK);
    let count = match last.as_ref() {
        Some(c)
            if c.node == node
                && now.duration_since(c.at) < MULTI_CLICK
                && (c.x - x).abs() < 4.0
                && (c.y - y).abs() < 4.0 =>
        {
            c.count.saturating_add(1)
        }
        _ => 1,
    };
    *last = Some(Click { at: now, node, x, y, count });
    *lock(&DRAG) = (count == 1).then_some(node);
    count
}

/// The input a drag selection is going on in.
pub(crate) fn dragging() -> Option<usize> {
    *lock(&DRAG)
}

/// The mouse button was released over `target`. Returns whether the click
/// must not happen: the press started a selection in another element.
pub(crate) fn release(target: usize) -> bool {
    lock(&DRAG).take().is_some_and(|input| input != target)
}

/// Before a right click goes to the page: remember the input under it.
pub(crate) fn note_context(doc: &BaseDocument, hovered: Option<usize>) {
    let context = hovered.and_then(|id| {
        let input = doc.get_node(id)?.element_data()?.text_input_data()?;
        let focused = doc.focus_node_id == Some(id);
        Some((id, focused && input.editor.selected_text().is_some_and(|t| !t.is_empty())))
    });
    *lock(&CONTEXT) = context;
}

/// Whether the last right click was on a text input, and if so whether it
/// had selected text.
pub fn context() -> Option<bool> {
    lock(&CONTEXT).map(|(_, selected)| selected)
}

/// Asks for an edit on the input of the last right click. The shell runs it
/// at its next poll (wake it, e.g. with `canvas_frames::request_frame`).
pub fn request(edit: Edit) {
    if let Some((node, _)) = *lock(&CONTEXT) {
        *lock(&PENDING) = Some((node, edit));
    }
}

/// The edit to run, if any.
pub fn take_request() -> Option<(usize, Edit)> {
    lock(&PENDING).take()
}

/// Scrolls a textarea taller than its box so the caret stays in view.
pub(crate) fn keep_caret_visible(doc: &mut BaseDocument, node_id: usize) {
    let scale = doc.viewport.scale_f64();
    let Some(node) = doc.nodes.get_mut(node_id) else { return };
    let layout = node.final_layout;
    let Some(input) = node.element_data().and_then(|e| e.text_input_data()) else { return };
    if !input.is_multiline {
        return;
    }
    let Some(text) = input.editor.try_layout() else { return };
    let caret = input.editor.raw_selection().focus().geometry(text, 1.0);
    let (top, bottom) = (caret.y0 / scale, caret.y1 / scale);
    let visible = (layout.size.height - layout.padding.top - layout.padding.bottom - layout.border.top - layout.border.bottom) as f64;
    let max = layout.scroll_height() as f64;
    let mut scroll = node.scroll_offset.y;
    if top < scroll {
        scroll = top;
    } else if bottom > scroll + visible {
        scroll = bottom - visible;
    }
    node.scroll_offset.y = scroll.clamp(0.0, max.max(0.0));
}

/// Asks to focus the element with this `id` at the shell's next poll.
pub fn request_focus(id: &str) {
    *lock(&FOCUS) = Some(id.to_owned());
}

impl BaseDocument {
    /// Focuses an element the app asked for, if it exists.
    pub fn apply_focus_request(&mut self) -> bool {
        let Some(id) = lock(&FOCUS).take() else { return false };
        match self.nodes_to_id.get(&id).copied() {
            Some(node) => self.set_focus_to(node),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_count_up_and_drags_belong_to_their_input() {
        assert_eq!(press(7, 10.0, 10.0), 1);
        assert_eq!(dragging(), Some(7));
        assert_eq!(press(7, 11.0, 10.0), 2);
        assert_eq!(dragging(), None, "a double click doesn't drag");
        assert_eq!(press(7, 11.0, 11.0), 3);
        // Elsewhere starts over.
        assert_eq!(press(7, 60.0, 10.0), 1);
        // Released over another element: that one isn't clicked.
        assert!(release(9));
        assert_eq!(press(7, 200.0, 10.0), 1);
        assert!(!release(7));
    }
}
