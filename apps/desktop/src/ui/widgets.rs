//! Small controls Blitz has no native version of: switches, sliders,
//! dropdowns, segmented buttons, meters, plus avatars.

use dioxus_native::prelude::*;
use sivryn_client_core::protocol::model::UserStatus;
use sivryn_client_core::protocol::{FileId, UserId};

use super::icons::{Icon, Svg};

/// A user's profile picture, or colorful initials (the color derived from
/// the user ID so it's stable). `speaking` draws a green ring around it.
#[component]
pub fn Avatar(
    name: String,
    #[props(default)] id: Option<UserId>,
    #[props(default = 36)] size: u32,
    #[props(default)] speaking: bool,
) -> Element {
    const PALETTE: [&str; 8] = ["#7c6cff", "#3ba7ff", "#2fbf8f", "#f2994a", "#eb5a8c", "#9b6cff", "#16b5c8", "#e2b93b"];
    let state = use_context::<super::AppState>();
    let picture = id.and_then(|id| state.avatars.read().get(&id).copied());
    let seed = id.map(|id| id.0.as_u128() as usize).unwrap_or_else(|| name.bytes().map(usize::from).sum());
    let color = PALETTE[seed % PALETTE.len()];
    let initials: String = name.chars().filter(|c| c.is_alphanumeric()).take(2).collect::<String>().to_uppercase();
    let font = size * 2 / 5;
    let class = if speaking { "avatar speaking" } else { "avatar" };
    let background = if picture.is_some() { "transparent" } else { color };
    rsx! {
        div {
            class: "{class}",
            style: "width: {size}px; height: {size}px; min-width: {size}px; font-size: {font}px; background: {background};",
            // Keys only work in lists: a new picture must get a new canvas.
            for file in picture.iter() {
                RemoteImage { key: "{file}", file: *file, size: size * 2, round: true }
            }
            if picture.is_none() {
                "{initials}"
            }
        }
    }
}

/// A public image (avatar, banner, server icon) filling its parent.
#[component]
pub fn RemoteImage(file: FileId, size: u32, #[props(default)] round: bool) -> Element {
    let state = use_context::<super::AppState>();
    let frames = use_resource(use_reactive!(|file, size, round| async move {
        let s = state.session.cloned()?;
        let bytes = crate::files::fetch_plain(&s, file).await.ok()?;
        let decoded = tokio::task::spawn_blocking(move || {
            let mut d = crate::files::decode(&bytes, size, size).ok()?;
            if round {
                crate::files::round(&mut d);
            }
            Some(d)
        })
        .await
        .ok()??;
        Some((file, crate::files::play(decoded)))
    }));
    let loaded = frames.read().clone().flatten();
    rsx! {
        for (id, frames) in loaded {
            super::media::FrameCanvas { key: "{id}", frames: super::media::Frames(frames) }
        }
    }
}

/// A presence dot in the status's color; `None` is offline.
#[component]
pub fn StatusDot(status: Option<UserStatus>, #[props(default)] class: String) -> Element {
    let kind = match status {
        Some(UserStatus::Online) => "online",
        Some(UserStatus::Idle) => "idle",
        Some(UserStatus::Dnd) => "dnd",
        Some(UserStatus::Invisible) | None => "offline",
    };
    rsx! { div { class: "presence-dot {kind} {class}" } }
}

#[component]
pub fn Toggle(on: bool, onchange: EventHandler<bool>) -> Element {
    rsx! {
        div {
            class: if on { "toggle on" } else { "toggle" },
            onclick: move |_| onchange.call(!on),
            div { class: "knob" }
        }
    }
}

/// Horizontal slider. Blitz reports no element-relative mouse positions, so
/// the track is covered by invisible hit zones, one per step: clicking or
/// dragging over a zone selects its value.
#[component]
pub fn Slider(
    value: f32,
    min: f32,
    max: f32,
    #[props(default = 50)] steps: u32,
    onchange: EventHandler<f32>,
    /// Optional second marker, e.g. the live microphone level behind a threshold.
    #[props(default)]
    level: Option<f32>,
) -> Element {
    let frac = ((value - min) / (max - min)).clamp(0.0, 1.0);
    let pct = frac * 100.0;
    let level_pct = level.map(|l| ((l - min) / (max - min)).clamp(0.0, 1.0) * 100.0);
    let mut dragging = use_signal(|| false);
    let value_at = move |i: u32| min + (max - min) * (i as f32 / steps as f32);

    rsx! {
        div {
            class: "slider",
            onmouseup: move |_| dragging.set(false),
            onmouseleave: move |_| dragging.set(false),
            div { class: "slider-track",
                if let Some(level) = level_pct {
                    div { class: "slider-level", style: "width: {level}%;" }
                }
                div { class: "slider-fill", style: "width: {pct}%;" }
            }
            div { class: "slider-thumb", style: "left: calc({pct}% - 8px);" }
            div { class: "slider-hit",
                for i in 0..=steps {
                    div {
                        key: "{i}",
                        onmousedown: move |_| {
                            dragging.set(true);
                            onchange.call(value_at(i));
                        },
                        onmouseenter: move |_| {
                            if *dragging.peek() {
                                onchange.call(value_at(i));
                            }
                        },
                    }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

/// Dropdown list.
#[component]
pub fn Select(value: String, options: Vec<SelectOption>, onchange: EventHandler<String>) -> Element {
    let mut open = use_signal(|| false);
    let current = options.iter().find(|o| o.value == value).map(|o| o.label.clone()).unwrap_or_default();
    rsx! {
        div { class: "select",
            div { class: if *open.read() { "select-button open" } else { "select-button" }, onclick: move |_| open.toggle(),
                span { class: "select-value", "{current}" }
                Svg { icon: Icon::ChevronDown, size: 16 }
            }
            if *open.read() {
                div { class: "select-menu",
                    for option in options.iter() {
                        div {
                            key: "{option.value}",
                            class: if option.value == value { "select-option selected" } else { "select-option" },
                            onclick: {
                                let v = option.value.clone();
                                move |_| {
                                    open.set(false);
                                    onchange.call(v.clone());
                                }
                            },
                            span { "{option.label}" }
                            if option.value == value {
                                Svg { icon: Icon::Check, size: 16 }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A row of mutually exclusive buttons.
#[component]
pub fn Segmented(value: String, options: Vec<SelectOption>, onchange: EventHandler<String>) -> Element {
    rsx! {
        div { class: "segmented",
            for option in options.iter() {
                div {
                    key: "{option.value}",
                    class: if option.value == value { "seg active" } else { "seg" },
                    onclick: {
                        let v = option.value.clone();
                        move |_| onchange.call(v.clone())
                    },
                    "{option.label}"
                }
            }
        }
    }
}

/// Round icon button used in panels and the call controls.
#[component]
pub fn IconButton(
    icon: Icon,
    onclick: EventHandler<MouseEvent>,
    #[props(default)] class: String,
    #[props(default)] title: String,
    #[props(default = 20)] size: u32,
) -> Element {
    rsx! {
        div { class: "icon-button {class}", title: "{title}", onclick: move |e| onclick.call(e),
            Svg { icon, size }
        }
    }
}

pub fn opt(value: impl Into<String>, label: impl Into<String>) -> SelectOption {
    SelectOption { value: value.into(), label: label.into() }
}
