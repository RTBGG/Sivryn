//! Small controls Blitz has no native version of: switches, sliders,
//! dropdowns, segmented buttons, meters, plus avatars.

use dioxus_native::prelude::*;
use sivryn_client_core::protocol::model::UserStatus;
use sivryn_client_core::protocol::{FileId, UserId};

use super::icons::{Icon, Svg};

// --- Hover from mouse moves ------------------------------------------------------
//
// Blitz's `:hover` can't drive component state, and `mouseenter`/`mouseleave`
// are unreliable. Instead, hover-aware elements mark themselves on every
// mouse move that passes through them, and the window root (which sees the
// move last) turns exactly the marked ones on and the rest off.

thread_local! {
    static HOVER_MARKS: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
    static HOVERED: std::cell::RefCell<Vec<(u64, Signal<bool>)>> = const { std::cell::RefCell::new(Vec::new()) };
    static HOVER_FLAGS: std::cell::RefCell<Vec<(u64, Signal<bool>)>> = const { std::cell::RefCell::new(Vec::new()) };
}
static NEXT_HOVER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
thread_local! {
    /// When an element was last marked as hovered.
    static LAST_MARKED: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}
const HOVER_GRACE: std::time::Duration = std::time::Duration::from_millis(40);

/// Sets a component's signal from a task that outlives it (`spawn_forever`
/// for a server request): when the component is gone by then, e.g. rebuilt
/// by the server's change event, there is nothing to update. `set` would
/// panic on the dropped signal.
pub fn set_if_alive<T: 'static>(mut signal: Signal<T>, value: T) {
    if let Ok(mut current) = signal.try_write() {
        *current = value;
    }
}

/// Whether the element is hovered, and the handler to put on its `onmousemove`.
pub fn use_hover() -> (bool, impl FnMut(MouseEvent) + Clone + 'static) {
    let flag = use_signal(|| false);
    let id = use_hook(move || {
        let id = NEXT_HOVER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        HOVER_FLAGS.with(|f| f.borrow_mut().push((id, flag)));
        id
    });
    use_drop(move || {
        HOVER_FLAGS.with(|f| f.borrow_mut().retain(|(i, _)| *i != id));
        HOVERED.with(|h| h.borrow_mut().retain(|(i, _)| *i != id));
    });
    let mark = move |_: MouseEvent| HOVER_MARKS.with(|m| m.borrow_mut().push(id));
    (*flag.read(), mark)
}

/// Called by the window root after every mouse move.
pub fn hover_root_move() {
    let marks = HOVER_MARKS.with(|m| std::mem::take(&mut *m.borrow_mut()));
    // Blitz delivers some moves twice or three times to the root, the extra
    // copies without passing the elements under the pointer; they must not
    // end the hover. A hover ends once no element was marked for a moment.
    if marks.is_empty() && LAST_MARKED.with(|t| t.get()).is_some_and(|t| t.elapsed() < HOVER_GRACE) {
        return;
    }
    if !marks.is_empty() {
        LAST_MARKED.with(|t| t.set(Some(std::time::Instant::now())));
    }
    HOVERED.with(|h| {
        let mut hovered = h.borrow_mut();
        hovered.retain(|(id, flag)| {
            let keep = marks.contains(id);
            if !keep {
                let mut flag = *flag;
                flag.set(false);
            }
            keep
        });
        for id in marks {
            if hovered.iter().any(|(i, _)| *i == id) {
                continue;
            }
            if let Some((_, flag)) = HOVER_FLAGS.with(|f| f.borrow().iter().find(|(i, _)| *i == id).copied()) {
                let mut f = flag;
                f.set(true);
                hovered.push((id, flag));
            }
        }
    });
}

/// A user's profile picture, or colorful initials (the color derived from
/// the user ID so it's stable). `speaking` draws a green ring around it.
/// Animated pictures play while hovered (or while `animate` is set, e.g.
/// when the row they're in is hovered).
#[component]
pub fn Avatar(
    name: String,
    #[props(default)] id: Option<UserId>,
    #[props(default = 36)] size: u32,
    #[props(default)] speaking: bool,
    #[props(default)] animate: bool,
) -> Element {
    let (hovered, mark) = use_hover();
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
            onmousemove: mark,
            // Keys only work in lists: a new picture must get a new canvas.
            for file in picture.iter() {
                RemoteImage { key: "{file}", file: *file, size: size * 2, round: true, animate: animate || hovered }
            }
            if picture.is_none() {
                "{initials}"
            }
        }
    }
}

/// A public image (avatar, banner, server icon) filling its parent. An
/// animated one only plays while `animate` is set.
#[component]
pub fn RemoteImage(
    file: FileId,
    size: u32,
    #[props(default)] round: bool,
    #[props(default)] animate: bool,
    /// Fill the parent, cropping the overflow (banners).
    #[props(default)]
    cover: bool,
) -> Element {
    let state = use_context::<super::AppState>();
    let gate = use_hook(move || std::rc::Rc::new(tokio::sync::watch::channel(animate)));
    if *gate.0.borrow() != animate {
        let _ = gate.0.send(animate);
    }
    let gate_rx = gate.1.clone();
    let frames = use_resource(use_reactive!(|file, size, round| {
        let gate_rx = gate_rx.clone();
        async move {
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
            Some((file, crate::files::play_gated(decoded, gate_rx)))
        }
    }));
    let loaded = frames.read().clone().flatten();
    rsx! {
        for (id, frames) in loaded {
            super::media::FrameCanvas { key: "{id}", frames: super::media::Frames(frames), cover }
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

/// The slider being dragged, if any. The window root forwards every mouse
/// move to it ([`slider_pointer_move`]), so a drag keeps going when the
/// pointer leaves the slider, and can go past its ends to the minimum or
/// maximum.
struct SliderDrag {
    /// The zone pressed first and the pointer's x there.
    start: (u32, f64),
    /// A zone crossed later (as far from the first as seen so far): with the
    /// first it tells how many pixels one step is.
    calibration: Option<(u32, f64)>,
    min: f32,
    max: f32,
    steps: u32,
    onchange: EventHandler<f32>,
    onrelease: Option<EventHandler<()>>,
}

thread_local! {
    static SLIDER_DRAG: std::cell::RefCell<Option<SliderDrag>> = const { std::cell::RefCell::new(None) };
}

/// A mouse move anywhere in the window (`held`: the left button is down).
pub fn slider_pointer_move(x: f64, held: bool) {
    if !held {
        slider_pointer_up();
        return;
    }
    let update = SLIDER_DRAG.with(|d| {
        let d = d.borrow();
        let d = d.as_ref()?;
        let (ci, cx) = d.calibration?;
        let px_per_step = (cx - d.start.1) / (ci as f64 - d.start.0 as f64);
        if !px_per_step.is_finite() || px_per_step.abs() < 0.01 {
            return None;
        }
        let step = (d.start.0 as f64 + (x - d.start.1) / px_per_step).clamp(0.0, d.steps as f64);
        Some((d.onchange, d.min + (d.max - d.min) * (step / d.steps as f64) as f32))
    });
    if let Some((onchange, value)) = update {
        onchange.call(value);
    }
}

/// The mouse button went up anywhere in the window.
pub fn slider_pointer_up() {
    let ended = SLIDER_DRAG.with(|d| d.borrow_mut().take());
    if let Some(onrelease) = ended.and_then(|d| d.onrelease) {
        onrelease.call(());
    }
}

/// Horizontal slider. Blitz reports no element-relative mouse positions, so
/// the track is covered by invisible hit zones, one per step: pressing one
/// selects its value. While dragging, the zones the pointer crosses measure
/// the step width, and from then on the value follows the pointer's x
/// anywhere in the window (see [`slider_pointer_move`]).
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
    /// Called when a drag ends.
    #[props(default)]
    onrelease: Option<EventHandler<()>>,
) -> Element {
    let frac = ((value - min) / (max - min)).clamp(0.0, 1.0);
    let pct = frac * 100.0;
    let level_pct = level.map(|l| ((l - min) / (max - min)).clamp(0.0, 1.0) * 100.0);
    let value_at = move |i: u32| min + (max - min) * (i as f32 / steps as f32);

    rsx! {
        div { class: "slider",
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
                        onmousedown: move |e| {
                            let x = e.client_coordinates().x;
                            SLIDER_DRAG.with(|d| {
                                *d.borrow_mut() = Some(SliderDrag {
                                    start: (i, x),
                                    calibration: None,
                                    min,
                                    max,
                                    steps,
                                    onchange,
                                    onrelease,
                                })
                            });
                            onchange.call(value_at(i));
                        },
                        onmousemove: move |e| {
                            let x = e.client_coordinates().x;
                            let first = SLIDER_DRAG.with(|d| {
                                let mut d = d.borrow_mut();
                                let Some(d) = d.as_mut() else { return false };
                                // The crossing farthest from the start measures best.
                                let far = d.calibration.is_none_or(|(c, _)| i.abs_diff(d.start.0) > c.abs_diff(d.start.0));
                                if i != d.start.0 && far {
                                    let first = d.calibration.is_none();
                                    d.calibration = Some((i, x));
                                    return first;
                                }
                                false
                            });
                            // Until the window root takes over, follow the zones.
                            if first {
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
