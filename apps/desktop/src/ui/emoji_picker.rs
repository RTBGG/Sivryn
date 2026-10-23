//! Emoji picker for reactions and the message box.

use dioxus_native::prelude::*;
use sivryn_client_core::protocol::{ChannelId, MessageId};

use super::AppState;
use super::icons::{Icon, Svg};
use crate::emoji::{self, GROUPS};
use crate::i18n::t;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PickerTarget {
    /// React to a message.
    React(ChannelId, MessageId),
    /// Insert into a channel's message box.
    Composer(ChannelId),
}

pub fn open_for_reaction(mut state: AppState, channel: ChannelId, message: MessageId) {
    state.picker.set(Some(PickerTarget::React(channel, message)));
}

pub fn open_for_composer(mut state: AppState, channel: ChannelId) {
    state.picker.set(Some(PickerTarget::Composer(channel)));
}

fn choose(mut state: AppState, target: PickerTarget, emoji: &str) {
    match target {
        PickerTarget::React(channel, message) => {
            state.picker.set(None);
            super::chat_view::toggle_reaction(state, channel, message, emoji.to_owned());
        }
        // Stays open, so several emoji can be added in a row.
        PickerTarget::Composer(channel) => {
            state.composer.write().entry(channel).or_default().draft.push_str(emoji);
            // Back into the message box, right after the emoji.
            super::chat_view::focus_composer(channel);
        }
    }
}

#[component]
pub fn EmojiPicker() -> Element {
    let mut state = use_context::<AppState>();
    let mut query = use_signal(String::new);
    let mut group = use_signal(|| GROUPS[0].0);
    let Some(target) = *state.picker.read() else { return rsx! {} };

    let q = query.read().clone();
    let results: Vec<emoji::Emoji> =
        if q.trim().is_empty() { emoji::in_group(*group.read()).collect() } else { emoji::search(&q) };

    rsx! {
        div { class: "menu-backdrop", onmousedown: move |_| state.picker.set(None) }
        div { class: "emoji-picker", onmousedown: move |e| e.stop_propagation(),
            div { class: "emoji-search",
                Svg { icon: Icon::Search, size: 16 }
                input {
                    autofocus: "true",
                    value: "{query}",
                    placeholder: t("emoji.search"),
                    oninput: move |e| query.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Escape {
                            state.picker.set(None);
                        }
                    },
                }
            }
            if q.trim().is_empty() {
                div { class: "emoji-quick",
                    for e in emoji::QUICK {
                        div { key: "{e}", class: "emoji-cell emoji", onclick: move |_| choose(state, target, e), "{e}" }
                    }
                }
            }
            div { class: "emoji-grid",
                if results.is_empty() {
                    div { class: "muted emoji-empty", {t("emoji.none")} }
                }
                for e in results {
                    div {
                        key: "{e.emoji}",
                        class: "emoji-cell emoji",
                        title: "{e.name}",
                        onclick: move |_| choose(state, target, e.emoji),
                        "{e.emoji}"
                    }
                }
            }
            div { class: "emoji-groups",
                for (g, icon) in GROUPS {
                    div {
                        key: "{g}",
                        class: if *group.read() == g && q.trim().is_empty() { "emoji-group active emoji" } else { "emoji-group emoji" },
                        onclick: move |_| {
                            query.set(String::new());
                            group.set(g);
                        },
                        "{icon}"
                    }
                }
            }
        }
    }
}
