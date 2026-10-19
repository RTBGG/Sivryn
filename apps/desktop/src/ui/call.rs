//! Voice channel UI: join prompt, participant grid, controls, per-user
//! volume and the screen picker.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use dioxus_native::use_wgpu;
use sivryn_client_core::protocol::ChannelId;
use sivryn_client_core::protocol::media::{ContentHint, VideoCodec};
use sivryn_media::{
    CallEvent, CameraShare, LoopbackTarget, ScreenAudio, ScreenShare, ScreenSource, Sound, SourceKind, TrackSource,
    VideoFeed, VoiceCall, list_sources, screen_volume_key, window_process,
};

use super::icons::{Icon, Svg};
use super::widgets::{Avatar, IconButton, Slider, Toggle};
use super::{AppState, CallState, Modal, Participant, Services};
use crate::chat;
use crate::i18n::t;
use crate::settings::ScreenQuality;
use crate::video_surface::VideoSurface;

#[component]
pub fn JoinPrompt(channel: ChannelId) -> Element {
    let state = use_context::<AppState>();
    let services = use_context::<Services>();
    let mut joining = use_signal(|| false);
    let name = state.channel_name(channel);
    let present = state.voice.read().get(&channel).cloned().unwrap_or_default();

    let join = move |_| {
        if *joining.peek() {
            return;
        }
        joining.set(true);
        let engine = services.engine.clone();
        // Runs in the root scope: this component unmounts as soon as the call
        // view takes over, which would cancel a task spawned here.
        spawn_forever(async move {
            let result = connect(state, channel, engine).await;
            joining.set(false);
            if let Err(e) = result {
                state.report(Err(e));
            }
        });
    };

    rsx! {
        div { class: "join",
            div { class: "join-card",
                div { class: "join-icon", Svg { icon: Icon::Volume, size: 34 } }
                h2 { "{name}" }
                if present.is_empty() {
                    p { class: "muted", {t("call.nobody_here")} }
                } else {
                    div { class: "join-present",
                        for p in present.iter() {
                            Avatar { key: "{p.user_id}", name: state.username(p.user_id), id: Some(p.user_id), size: 32 }
                        }
                    }
                    p { class: "muted", "{present.len()} {t(\"call.people_here\")}" }
                }
                button { class: if *joining.read() { "button primary large busy" } else { "button primary large" }, onclick: join,
                    if *joining.read() { {t("call.connecting")} } else { {t("call.join")} }
                }
                p { class: "join-hint", {t("call.quality_hint")} }
            }
        }
    }
}

async fn connect(mut state: AppState, channel: ChannelId, engine: sivryn_media::AudioEngine) -> Result<(), String> {
    let Some(s) = state.session.cloned() else { return Ok(()) };
    let guild = state.guild_of(channel).ok_or("unknown channel")?;
    if let Some(old) = state.call.take() {
        leave(old).await;
    }
    let join = s.api.join_voice(channel).await.map_err(|e| e.to_string())?;
    let media_key = if join.e2ee {
        if !chat::ensure_group(&s, guild, channel).await? {
            return Err(t("call.waiting_for_key").into());
        }
        Some(s.mls.borrow().media_key(channel).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let (mut call, mut events) = VoiceCall::join(&join, media_key, engine).await.map_err(|e| e.to_string())?;
    if let Err(e) = call.enable_microphone().await {
        state.report(Err(format!("{}: {e}", t("call.mic_unavailable"))));
    }
    let silenced = *state.muted.peek() || *state.deafened.peek();
    call.set_muted(silenced);

    let identity = call.identity();
    let me = Participant { identity: identity.clone(), name: s.user.username.clone(), muted: silenced, sharing: false };
    state.call.set(Some(CallState {
        channel,
        join,
        call: Rc::new(RefCell::new(Some(call))),
        share: Rc::new(RefCell::new(None)),
        share_audio: Rc::new(RefCell::new(None)),
        camera: Rc::new(RefCell::new(None)),
        identity,
        participants: vec![me],
        feeds: Vec::new(),
        speakers: Vec::new(),
        sharing: false,
        camera_on: false,
        focus: None,
    }));
    spawn_forever(state.refresh_voice());

    spawn_forever(async move {
        while let Some(event) = events.recv().await {
            // Look up names before taking the write lock on the call.
            let joined_name = match &event {
                CallEvent::ParticipantJoined { identity, name } => {
                    Some(identity.parse().ok().map(|id| state.username(id)).unwrap_or_else(|| name.clone()))
                }
                _ => None,
            };
            let mut call = state.call.write();
            let Some(c) = call.as_mut().filter(|c| c.channel == channel) else { break };
            match event {
                CallEvent::VideoAdded(feed) => c.feeds.push(feed),
                CallEvent::VideoRemoved { track_sid } => {
                    c.feeds.retain(|f| f.track_sid != track_sid);
                    if c.focus.as_deref() == Some(track_sid.as_str()) {
                        c.focus = None;
                    }
                }
                CallEvent::SpeakingChanged { speakers } => c.speakers = speakers,
                CallEvent::ParticipantJoined { identity, .. } => {
                    if !c.participants.iter().any(|p| p.identity == identity) {
                        let name = joined_name.unwrap_or_default();
                        c.participants.push(Participant { identity, name, muted: false, sharing: false });
                    }
                }
                CallEvent::ParticipantLeft { identity } => c.participants.retain(|p| p.identity != identity),
                CallEvent::MuteChanged { identity, muted } => {
                    if let Some(p) = c.participants.iter_mut().find(|p| p.identity == identity) {
                        p.muted = muted;
                    }
                }
                CallEvent::ShareChanged { identity, sharing } => {
                    if let Some(p) = c.participants.iter_mut().find(|p| p.identity == identity) {
                        p.sharing = sharing;
                    }
                }
                CallEvent::Disconnected => {
                    *call = None;
                    drop(call);
                    state.toast(t("call.disconnected"), true);
                    spawn_forever(state.refresh_voice());
                    break;
                }
            }
        }
    });
    Ok(())
}

pub async fn leave(call: CallState) {
    let camera = call.camera.borrow_mut().take();
    if let Some(camera) = camera {
        camera.stop().await;
    }
    let audio = call.share_audio.borrow_mut().take();
    if let Some(audio) = audio {
        audio.stop().await;
    }
    let share = call.share.borrow_mut().take();
    if let Some(share) = share {
        share.stop().await;
    }
    let vc = call.call.borrow_mut().take();
    if let Some(vc) = vc {
        vc.leave().await;
    }
}

pub fn hang_up(mut state: AppState) {
    if let Some(call) = state.call.take() {
        // The call view unmounts right away; leaving must still finish.
        spawn_forever(async move {
            leave(call).await;
            state.refresh_voice().await;
        });
    }
}

pub fn stop_share(mut state: AppState) {
    let parts = state.call.write().as_mut().map(|c| {
        c.sharing = false;
        let me = c.identity.clone();
        if let Some(p) = c.participants.iter_mut().find(|p| p.identity == me) {
            p.sharing = false;
        }
        (c.share.borrow_mut().take(), c.share_audio.borrow_mut().take())
    });
    if let Some((share, audio)) = parts {
        spawn_forever(async move {
            if let Some(audio) = audio {
                audio.stop().await;
            }
            if let Some(share) = share {
                share.stop().await;
            }
        });
    }
}

pub fn toggle_camera(mut state: AppState) {
    let Some(c) = state.call.cloned() else { return };
    if c.camera_on {
        let camera = c.camera.borrow_mut().take();
        if let Some(call) = state.call.write().as_mut() {
            call.camera_on = false;
            if let Some(cam) = &camera {
                let sid = cam.preview.track_sid.clone();
                call.feeds.retain(|f| f.track_sid != sid);
            }
        }
        if let Some(camera) = camera {
            spawn_forever(camera.stop());
        }
        return;
    }
    let Some(room) = c.call.borrow().as_ref().map(|vc| vc.room.clone()) else { return };
    let video = state.settings.peek().video.clone();
    let limits = c.join.camera.clone();
    if let Some(call) = state.call.write().as_mut() {
        call.camera_on = true;
    }
    spawn_forever(async move {
        match CameraShare::start(room, video.camera.as_deref(), video.camera_quality, &limits).await {
            Ok(camera) => {
                let preview = camera.preview.clone();
                *c.camera.borrow_mut() = Some(camera);
                if let Some(call) = state.call.write().as_mut() {
                    call.feeds.push(preview);
                }
            }
            Err(e) => {
                if let Some(call) = state.call.write().as_mut() {
                    call.camera_on = false;
                }
                let reason = match e {
                    sivryn_media::MediaError::NoCamera => t("video.no_camera").to_string(),
                    sivryn_media::MediaError::NoMediaFoundation => t("video.no_mf").to_string(),
                    other => other.to_string(),
                };
                state.report(Err(format!("{}: {reason}", t("call.camera_failed"))));
            }
        }
    });
}

#[component]
pub fn CallView() -> Element {
    let mut state = use_context::<AppState>();
    let call = state.call.read();
    let Some(c) = call.as_ref() else { return rsx! {} };
    let name = state.channel_name(c.channel);

    // Screen shares can be focused; the focused one is shown large.
    let screens: Vec<VideoFeed> = c.feeds.iter().filter(|f| f.source == TrackSource::Screenshare).cloned().collect();
    let focused = c
        .focus
        .as_ref()
        .and_then(|sid| c.feeds.iter().find(|f| &f.track_sid == sid))
        .or_else(|| screens.first())
        .cloned();
    let participants = c.participants.clone();
    let speakers = c.speakers.clone();
    let cameras: Vec<VideoFeed> = c.feeds.iter().filter(|f| f.source == TrackSource::Camera).cloned().collect();
    let other_screens: Vec<VideoFeed> =
        screens.iter().filter(|f| Some(&f.track_sid) != focused.as_ref().map(|x| &x.track_sid)).cloned().collect();
    let (sharing, camera_on) = (c.sharing, c.camera_on);
    drop(call);
    let muted = *state.muted.read() || *state.deafened.read();
    let deafened = *state.deafened.read();
    let count = participants.len();

    rsx! {
        div { class: "call",
            header { class: "topbar",
                span { class: "topbar-icon", Svg { icon: Icon::Volume, size: 20 } }
                span { class: "topbar-title", "{name}" }
                span { class: "e2ee-badge", Svg { icon: Icon::ShieldCheck, size: 14 } {t("call.e2ee")} }
                span { class: "topbar-spacer" }
                span { class: "muted small", "{count} {t(\"call.in_call\")}" }
            }
            div { class: if focused.is_some() { "stage focus" } else { "stage" },
                if let Some(feed) = focused.clone() {
                    div { class: "stage-main",
                        VideoTile { key: "{feed.track_sid}", feed, large: true }
                    }
                }
                div { class: "stage-grid",
                    for feed in other_screens {
                        VideoTile { key: "{feed.track_sid}", feed: feed.clone(), large: false }
                    }
                    for p in participants {
                        ParticipantTile {
                            key: "{p.identity}",
                            camera: cameras.iter().find(|f| f.participant == p.identity).cloned(),
                            speaking: speakers.contains(&p.identity),
                            participant: p,
                        }
                    }
                }
            }
            div { class: "controls",
                if muted {
                    IconButton { icon: Icon::MicOff, class: "control alert", title: t("call.unmute"), onclick: move |_| state.set_muted(false) }
                } else {
                    IconButton { icon: Icon::Mic, class: "control", title: t("call.mute"), onclick: move |_| state.set_muted(true) }
                }
                if deafened {
                    IconButton { icon: Icon::HeadphonesOff, class: "control alert", title: t("call.undeafen"), onclick: move |_| state.set_deafened(false) }
                } else {
                    IconButton { icon: Icon::Headphones, class: "control", title: t("call.deafen"), onclick: move |_| state.set_deafened(true) }
                }
                if camera_on {
                    IconButton { icon: Icon::Video, class: "control on", title: t("call.camera_off"), onclick: move |_| toggle_camera(state) }
                } else {
                    IconButton { icon: Icon::VideoOff, class: "control", title: t("call.camera_on"), onclick: move |_| toggle_camera(state) }
                }
                if sharing {
                    IconButton { icon: Icon::MonitorOff, class: "control on", title: t("call.stop_share"), onclick: move |_| stop_share(state) }
                } else {
                    IconButton { icon: Icon::MonitorUp, class: "control", title: t("call.share"), onclick: move |_| state.modal.set(Some(Modal::PickScreen)) }
                }
                IconButton { icon: Icon::PhoneOff, class: "control danger", title: t("call.leave"), onclick: move |_| hang_up(state) }
            }
        }
    }
}

/// A participant: their camera if it's on, otherwise their avatar.
#[component]
fn ParticipantTile(participant: Participant, camera: Option<VideoFeed>, speaking: bool) -> Element {
    let state = use_context::<AppState>();
    let mut menu = use_signal(|| false);
    let me = state.call.read().as_ref().is_some_and(|c| c.identity == participant.identity);
    let id = participant.identity.parse().ok();
    let class = if speaking { "tile participant speaking" } else { "tile participant" };

    rsx! {
        div { class: "{class}",
            onclick: move |_| {
                if !me {
                    menu.toggle();
                }
            },
            match camera {
                Some(feed) => rsx! { VideoCanvas { key: "{feed.track_sid}", feed } },
                None => rsx! {
                    div { class: "tile-avatar",
                        Avatar { name: participant.name.clone(), id, size: 76, speaking }
                    }
                },
            }
            div { class: "name-tag",
                if participant.muted {
                    span { class: "tag-muted", Svg { icon: Icon::MicOff, size: 13 } }
                }
                span { "{participant.name}" }
                if participant.sharing {
                    span { class: "live-badge", "LIVE" }
                }
            }
            if *menu.read() {
                VolumeMenu { identity: participant.identity.clone(), sharing: participant.sharing, onclose: move |_| menu.set(false) }
            }
        }
    }
}

/// Per-user (and per-share-audio) volume, saved in the settings.
#[component]
fn VolumeMenu(identity: String, sharing: bool, onclose: EventHandler<()>) -> Element {
    let state = use_context::<AppState>();
    let volumes = state.settings.read().volumes.clone();
    let voice_key = identity.clone();
    let screen_key = screen_volume_key(&identity);
    let voice = volumes.get(&voice_key).copied().unwrap_or(1.0);
    let screen = volumes.get(&screen_key).copied().unwrap_or(1.0);

    rsx! {
        div { class: "volume-menu", onclick: move |e| e.stop_propagation(),
            div { class: "menu-row",
                span { {t("call.user_volume")} }
                span { class: "muted small", "{(voice * 100.0).round()} %" }
            }
            Slider {
                value: voice,
                min: 0.0,
                max: 2.0,
                steps: 40,
                onchange: move |v: f32| {
                    let key = voice_key.clone();
                    state.update_settings(|s| {
                        s.volumes.insert(key, v);
                    });
                },
            }
            if sharing {
                div { class: "menu-row",
                    span { {t("call.stream_volume")} }
                    span { class: "muted small", "{(screen * 100.0).round()} %" }
                }
                Slider {
                    value: screen,
                    min: 0.0,
                    max: 2.0,
                    steps: 40,
                    onchange: move |v: f32| {
                        let key = screen_key.clone();
                        state.update_settings(|s| {
                            s.volumes.insert(key, v);
                        });
                    },
                }
            }
            div { class: "menu-close", onclick: move |_| onclose.call(()), {t("common.close")} }
        }
    }
}

#[component]
fn VideoTile(feed: VideoFeed, large: bool) -> Element {
    let mut state = use_context::<AppState>();
    let name = feed.participant.parse().map(|id| state.username(id)).unwrap_or(feed.participant.clone());
    let sid = feed.track_sid.clone();
    rsx! {
        div {
            class: if large { "tile screen large" } else { "tile screen" },
            onclick: move |_| {
                if let Some(c) = state.call.write().as_mut() {
                    c.focus = Some(sid.clone());
                }
            },
            VideoCanvas { feed }
            div { class: "name-tag",
                Svg { icon: Icon::Monitor, size: 13 }
                span { "{name}" }
            }
        }
    }
}

#[component]
fn VideoCanvas(feed: VideoFeed) -> Element {
    let frames = feed.frames.clone();
    let paint_source = use_wgpu(move || VideoSurface::new(frames));
    rsx! {
        canvas { class: "video", "src": "{paint_source}" }
    }
}

#[component]
pub fn ScreenPicker() -> Element {
    let mut state = use_context::<AppState>();
    let services = use_context::<Services>();
    let sources = use_hook(list_sources);
    let video = state.settings.read().video.clone();
    let mut starting = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    // The picker stays open until the share has started; closing it earlier
    // would cancel the task spawned here.
    let pick = use_callback(move |source: ScreenSource| {
        if *starting.read() {
            return;
        }
        let Some(c) = state.call.cloned() else { return };
        let Some(room) = c.call.borrow().as_ref().map(|vc| vc.room.clone()) else { return };
        let video = state.settings.peek().video.clone();
        let limits = video.screen_quality.apply(&c.join.screen_share);
        let engine = services.engine.clone();
        starting.set(true);
        error.set(None);
        spawn(async move {
            // TODO: collect decoder support from participants instead of assuming it.
            let viewers = [VideoCodec::Av1, VideoCodec::H264];
            let result = ScreenShare::start(room.clone(), &source, &limits, video.screen_hint, &viewers).await;
            match result {
                Ok(share) => {
                    tracing::info!(codec = ?share.codec, "screen share started");
                    *c.share.borrow_mut() = Some(share);
                    if video.screen_audio {
                        let target = match source.kind {
                            SourceKind::Window => window_process(source.id).map(LoopbackTarget::Process),
                            SourceKind::Screen => Some(LoopbackTarget::AllExcept(std::process::id())),
                        };
                        if let Some(target) = target {
                            match ScreenAudio::start(room, target).await {
                                Ok(audio) => *c.share_audio.borrow_mut() = Some(audio),
                                Err(e) => state.toast(format!("{}: {e}", t("share.audio_failed")), true),
                            }
                        }
                    }
                    if let Some(call) = state.call.write().as_mut() {
                        call.sharing = true;
                        let me = call.identity.clone();
                        if let Some(p) = call.participants.iter_mut().find(|p| p.identity == me) {
                            p.sharing = true;
                        }
                    }
                    engine.play(Sound::ShareStart);
                    starting.set(false);
                    state.modal.set(None);
                }
                Err(e) => {
                    starting.set(false);
                    error.set(Some(e.to_string()));
                }
            }
        });
    });

    let screens: Vec<ScreenSource> = sources.iter().filter(|s| s.kind == SourceKind::Screen).cloned().collect();
    let windows: Vec<ScreenSource> = sources.iter().filter(|s| s.kind == SourceKind::Window).cloned().collect();
    let quality_label = video.screen_quality.label();

    rsx! {
        div { class: "overlay", onclick: move |_| state.modal.set(None),
            div { class: "dialog wide", onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { {t("share.title")} }
                    IconButton { icon: Icon::X, size: 18, onclick: move |_| state.modal.set(None) }
                }
                div { class: "share-options",
                    div { class: "share-option",
                        span { class: "field-label", {t("share.optimize")} }
                        div { class: "segmented",
                            div {
                                class: if video.screen_hint == ContentHint::Detail { "seg active" } else { "seg" },
                                onclick: move |_| state.update_settings(|s| s.video.screen_hint = ContentHint::Detail),
                                {t("share.detail")}
                            }
                            div {
                                class: if video.screen_hint == ContentHint::Motion { "seg active" } else { "seg" },
                                onclick: move |_| state.update_settings(|s| s.video.screen_hint = ContentHint::Motion),
                                {t("share.motion")}
                            }
                        }
                    }
                    div { class: "share-option",
                        span { class: "field-label", {t("share.quality")} }
                        div { class: "segmented",
                            for q in ScreenQuality::ALL {
                                div {
                                    key: "{q:?}",
                                    class: if video.screen_quality == q { "seg active" } else { "seg" },
                                    onclick: move |_| state.update_settings(|s| s.video.screen_quality = q),
                                    "{q.label()}"
                                }
                            }
                        }
                    }
                    div { class: "share-option inline",
                        Toggle { on: video.screen_audio, onchange: move |v| state.update_settings(|s| s.video.screen_audio = v) }
                        span { {t("share.audio")} }
                    }
                }
                div { class: "source-section", {t("share.screens")} }
                div { class: "sources",
                    for source in screens {
                        div {
                            key: "{source.id}",
                            class: "source-card",
                            onclick: {
                                let source = source.clone();
                                move |_| pick.call(source.clone())
                            },
                            div { class: "source-icon", Svg { icon: Icon::Monitor, size: 26 } }
                            span { class: "source-title", "{source.title}" }
                        }
                    }
                }
                div { class: "source-section", {t("share.windows")} }
                div { class: "sources list",
                    for source in windows {
                        div {
                            key: "{source.id}",
                            class: "source-row",
                            onclick: {
                                let source = source.clone();
                                move |_| pick.call(source.clone())
                            },
                            Svg { icon: Icon::Maximize, size: 16 }
                            span { class: "source-title", "{source.title}" }
                        }
                    }
                }
                if *starting.read() {
                    div { class: "share-status", div { class: "spinner small" } span { "{t(\"share.starting\")} ({quality_label})" } }
                }
                if let Some(err) = error.read().as_ref() {
                    div { class: "form-error", "{err}" }
                }
            }
        }
    }
}
