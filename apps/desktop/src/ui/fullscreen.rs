//! A video or a call's stream over the whole screen: the window goes
//! borderless fullscreen and shows just the picture. The controls hide
//! while the mouse rests (or is elsewhere) and come back when it moves.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dioxus_native::prelude::*;
use sivryn_media::MediaPlayer;

use super::AppState;
use super::icons::{Icon, Svg};
use super::media::{FrameCanvas, Frames, gain, remember_volume, remembered_volume};
use super::widgets::{IconButton, Slider};
use crate::i18n::t;

/// How long the controls stay after the mouse last moved.
const IDLE: Duration = Duration::from_millis(2500);

/// What is shown fullscreen.
#[derive(Clone)]
pub enum FullView {
    /// A call's screen share or camera, by track.
    Feed(String),
    /// A chat video's player (shared with the video in the chat).
    Video(Rc<MediaPlayer>),
}

impl PartialEq for FullView {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FullView::Feed(a), FullView::Feed(b)) => a == b,
            (FullView::Video(a), FullView::Video(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

pub fn open(mut state: AppState, view: FullView) {
    state.full_view.set(Some(view));
    blitz_shell::fullscreen::set_fullscreen(true);
}

pub fn close(mut state: AppState) {
    if state.full_view.peek().is_some() {
        state.full_view.set(None);
        blitz_shell::fullscreen::set_fullscreen(false);
        blitz_shell::fullscreen::set_cursor_visible(true);
    }
}

/// The mouse pointer disappears together with the controls.
fn use_cursor_follows(visible: bool) {
    let mut shown = use_signal(|| true);
    if *shown.peek() != visible {
        shown.set(visible);
        blitz_shell::fullscreen::set_cursor_visible(visible);
    }
    use_drop(|| blitz_shell::fullscreen::set_cursor_visible(true));
}

/// Whether the mouse moved within the last moments, and the function to
/// call on every move.
pub fn use_activity() -> (bool, impl FnMut() + Clone + 'static) {
    let mut active = use_signal(|| true);
    let last = use_hook(|| Rc::new(Cell::new(Instant::now())));
    use_future({
        let last = last.clone();
        move || {
            let last = last.clone();
            async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    if *active.peek() && last.get().elapsed() >= IDLE {
                        active.set(false);
                    }
                }
            }
        }
    });
    let kick = move || {
        last.set(Instant::now());
        if !*active.peek() {
            active.set(true);
        }
    };
    (*active.read(), kick)
}

/// The fullscreen layer (rendered while `state.full_view` is set).
#[component]
pub fn FullscreenView() -> Element {
    let view = use_context::<AppState>().full_view.read().clone();
    match view {
        Some(FullView::Feed(sid)) => rsx! { FeedFull { key: "{sid}", sid } },
        Some(FullView::Video(player)) => rsx! { VideoFull { player: PlayerRef(player) } },
        None => rsx! {},
    }
}

/// A player as a prop: equal when it is the same player.
#[derive(Clone)]
struct PlayerRef(Rc<MediaPlayer>);

impl PartialEq for PlayerRef {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

#[component]
fn FeedFull(sid: String) -> Element {
    let state = use_context::<AppState>();
    let (active, mut kick) = use_activity();
    use_cursor_follows(active);
    let feed = state.call.read().as_ref().and_then(|c| c.feeds.iter().find(|f| f.track_sid == sid).cloned());
    // The stream ended (or the call): back to the window.
    use_effect(move || {
        let gone = state.call.read().as_ref().is_none_or(|c| !c.feeds.iter().any(|f| f.track_sid == sid));
        if gone {
            close(state);
        }
    });
    let Some(feed) = feed else { return rsx! {} };
    let guild = state.call.read().as_ref().and_then(|c| state.guild_of(c.channel));
    let name = feed.participant.parse().map(|id| state.display_name_in(guild, id)).unwrap_or(feed.participant.clone());

    rsx! {
        div { class: if active { "fullscreen" } else { "fullscreen idle" }, onmousemove: move |_| kick(),
            FrameCanvas { frames: Frames(feed.frames.clone()) }
            if active {
                div { class: "fullscreen-bar",
                    Svg { icon: Icon::Monitor, size: 16 }
                    span { class: "fullscreen-name", "{name}" }
                    span { class: "topbar-spacer" }
                    IconButton { icon: Icon::Minimize, size: 18, title: t("media.exit_fullscreen"), onclick: move |_| close(state) }
                }
            }
        }
    }
}

#[component]
fn VideoFull(player: PlayerRef) -> Element {
    let state = use_context::<AppState>();
    let (active, mut kick) = use_activity();
    let p = player.0.clone();
    let mut progress = use_signal(|| (p.position_ms(), p.duration_ms(), p.is_playing()));
    let mut scrub = use_signal(|| None::<f32>);
    let mut volume = use_signal(remembered_volume);
    use_future({
        let p = p.clone();
        move || {
            let p = p.clone();
            async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let now = (p.position_ms(), p.duration_ms(), p.is_playing());
                    if scrub.peek().is_none() && *progress.peek() != now {
                        progress.set(now);
                    }
                }
            }
        }
    });

    let toggle = {
        let p = p.clone();
        move |_| {
            let play = !p.is_playing();
            p.set_playing(play);
            progress.write().2 = play;
        }
    };
    let (pos, dur, playing) = *progress.read();
    let shown = scrub.read().unwrap_or(if dur > 0 { pos as f32 / dur as f32 } else { 0.0 });
    let pos = scrub.read().map_or(pos, |f| (f * dur as f32) as u64);
    let clock = |ms: u64| format!("{}:{:02}", ms / 60_000, (ms / 1000) % 60);
    let seek = {
        let p = p.clone();
        move |v: f32| {
            scrub.set(Some(v / 100.0));
            let dur = progress.peek().1;
            if dur > 0 {
                p.seek((dur as f32 * (v / 100.0).clamp(0.0, 1.0)) as u64);
            }
        }
    };
    let set_volume = {
        let p = p.clone();
        move |v: f32| {
            volume.set(v);
            remember_volume(v);
            p.set_volume(gain(v));
        }
    };
    // The controls stay while paused.
    let controls = active || !playing;
    use_cursor_follows(controls);
    let toggle_picture = toggle.clone();
    let mut mute = set_volume.clone();

    rsx! {
        div { class: if controls { "fullscreen" } else { "fullscreen idle" }, onmousemove: move |_| kick(),
            div { class: "fullscreen-picture", onclick: toggle_picture,
                FrameCanvas { frames: Frames(p.frames.clone()) }
            }
            if controls {
                div { class: "fullscreen-bar", onclick: move |e| e.stop_propagation(),
                    div { class: "icon-button", onclick: toggle,
                        if playing { Svg { icon: Icon::Pause, size: 18 } } else { Svg { icon: Icon::Play, size: 18 } }
                    }
                    Slider {
                        value: (shown * 100.0).clamp(0.0, 100.0),
                        min: 0.0,
                        max: 100.0,
                        steps: 400,
                        onchange: seek,
                        onrelease: move |_| {
                            if let Some(f) = *scrub.peek() {
                                let dur = progress.peek().1;
                                progress.write().0 = (f * dur as f32) as u64;
                            }
                            scrub.set(None);
                        },
                    }
                    span { class: "video-time", "{clock(pos)} / {clock(dur)}" }
                    div {
                        class: "icon-button",
                        onclick: move |_| {
                            let v = if *volume.peek() > 0.0 { 0.0 } else { 1.0 };
                            mute(v);
                        },
                        if *volume.read() > 0.0 { Svg { icon: Icon::Volume, size: 18 } } else { Svg { icon: Icon::VolumeX, size: 18 } }
                    }
                    div { class: "video-volume",
                        Slider { value: *volume.read(), min: 0.0, max: 1.0, steps: 50, onchange: set_volume }
                    }
                    IconButton { icon: Icon::Minimize, size: 18, title: t("media.exit_fullscreen"), onclick: move |_| close(state) }
                }
            }
        }
    }
}
