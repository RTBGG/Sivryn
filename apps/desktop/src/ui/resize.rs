//! Panes whose width can be dragged: the channel list and a voice
//! channel's chat. The window root forwards mouse moves while dragging, so
//! the layout follows the pointer live; the width is saved on release.

use dioxus_native::prelude::dioxus_elements::input_data::MouseButton;
use dioxus_native::prelude::*;

use super::AppState;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pane {
    /// The channel list (its handle is on its right edge).
    Sidebar,
    /// A voice channel's chat (its handle is on its left edge).
    VoiceChat,
}

impl Pane {
    fn limits(self) -> (f64, f64) {
        match self {
            Pane::Sidebar => (200.0, 480.0),
            Pane::VoiceChat => (280.0, 760.0),
        }
    }
}

thread_local! {
    /// The pane being resized, where the drag started and its width then.
    static RESIZING: std::cell::Cell<Option<(Pane, f64, f64)>> = const { std::cell::Cell::new(None) };
}

/// Current width of a pane.
pub fn width(state: &AppState, pane: Pane) -> f64 {
    let s = state.settings.read();
    let (min, max) = pane.limits();
    match pane {
        Pane::Sidebar => s.sidebar_width,
        Pane::VoiceChat => s.voice_chat_width,
    }
    .clamp(min, max)
}

/// A mouse move anywhere in the window.
pub fn on_move(mut state: AppState, e: &MouseEvent) {
    let Some((pane, start_x, start_width)) = RESIZING.with(|r| r.get()) else { return };
    if !e.held_buttons().contains(MouseButton::Primary) {
        finish(state);
        return;
    }
    let dx = e.client_coordinates().x - start_x;
    let (min, max) = pane.limits();
    let w = match pane {
        Pane::Sidebar => start_width + dx,
        Pane::VoiceChat => start_width - dx,
    }
    .clamp(min, max)
    .round();
    // Live, without saving the settings file on every move.
    let mut s = state.settings.write();
    match pane {
        Pane::Sidebar => s.sidebar_width = w,
        Pane::VoiceChat => s.voice_chat_width = w,
    }
}

/// The mouse button went up: save the width.
pub fn finish(state: AppState) {
    if RESIZING.with(|r| r.take()).is_some() {
        state.settings.peek().save();
    }
}

/// The strip on a pane's edge that resizes it.
#[component]
pub fn ResizeHandle(pane: Pane) -> Element {
    let state = use_context::<AppState>();
    let class = match pane {
        Pane::Sidebar => "resize-handle right",
        Pane::VoiceChat => "resize-handle left",
    };
    rsx! {
        div {
            class: "{class}",
            onmousedown: move |e| {
                if e.trigger_button() == Some(MouseButton::Primary) {
                    e.stop_propagation();
                    let start = (pane, e.client_coordinates().x, width(&state, pane));
                    RESIZING.with(|r| r.set(Some(start)));
                }
            },
        }
    }
}
