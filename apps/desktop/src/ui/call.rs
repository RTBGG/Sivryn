//! Calls: voice channels and DM calls, the participant grid, controls,
//! incoming calls, the AFK move and the screen picker.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use dioxus_native::use_wgpu;
use sivryn_client_core::protocol::ChannelId;
use sivryn_client_core::protocol::media::{ContentHint, VideoCodec};
use sivryn_media::{
    CallEvent, CameraShare, LoopbackTarget, ScreenAudio, ScreenShare, ScreenSource, Sound, SourceKind, SourcePreviews,
    TrackSource, VideoFeed, VoiceCall, list_sources, screen_volume_key, window_process,
};

use super::icons::{Icon, Svg};
use super::media::{FrameCanvas, Frames};
use super::widgets::{Avatar, IconButton, Toggle};
use super::{AppState, CallState, Focus, Modal, Participant, Services};
use crate::chat;
use crate::i18n::t;
use crate::settings::{SCREEN_FPS, ScreenResolution};
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
    if let Some(old) = state.call.take() {
        leave(old).await;
    }
    let join = s.api.join_voice(channel).await.map_err(|e| e.to_string())?;
    let media_key = if join.e2ee {
        if !chat::ensure_group(&s, channel).await? {
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
        auto_focus: true,
        strip_hidden: false,
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
                    if c.focus == Some(Focus::Screen(track_sid.clone())) {
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

/// The call grid with controls. `compact` is the version above a DM's chat.
#[component]
pub fn CallView(#[props(default)] compact: bool) -> Element {
    let mut state = use_context::<AppState>();
    let call = state.call.read();
    let Some(c) = call.as_ref() else { return rsx! {} };
    let name = state.channel_name(c.channel);
    let channel = c.channel;

    // One screen share or participant can be shown large; the rest go into
    // a strip below (which can be hidden).
    let screens: Vec<VideoFeed> = c.feeds.iter().filter(|f| f.source == TrackSource::Screenshare).cloned().collect();
    let focus = c
        .focus
        .clone()
        .filter(|f| match f {
            Focus::Screen(sid) => screens.iter().any(|s| &s.track_sid == sid),
            Focus::User(id) => c.participants.iter().any(|p| &p.identity == id),
        })
        .or_else(|| screens.first().filter(|_| c.auto_focus).map(|s| Focus::Screen(s.track_sid.clone())));
    let participants = c.participants.clone();
    let speakers = c.speakers.clone();
    let cameras: Vec<VideoFeed> = c.feeds.iter().filter(|f| f.source == TrackSource::Camera).cloned().collect();
    let strip_hidden = c.strip_hidden;
    let (sharing, camera_on) = (c.sharing, c.camera_on);
    let big_screen = match &focus {
        Some(Focus::Screen(sid)) => screens.iter().find(|s| &s.track_sid == sid).cloned(),
        _ => None,
    };
    let big_user = match &focus {
        Some(Focus::User(id)) => participants.iter().find(|p| &p.identity == id).cloned(),
        _ => None,
    };
    let other_screens: Vec<VideoFeed> =
        screens.iter().filter(|s| Some(&s.track_sid) != big_screen.as_ref().map(|b| &b.track_sid)).cloned().collect();
    let other_people: Vec<Participant> =
        participants.iter().filter(|p| Some(&p.identity) != big_user.as_ref().map(|b| &b.identity)).cloned().collect();
    drop(call);
    let muted = *state.muted.read() || *state.deafened.read();
    let deafened = *state.deafened.read();
    let count = participants.len();

    rsx! {
        div { class: if compact { "call compact" } else { "call" },
            if !compact {
            header { class: "topbar",
                span { class: "topbar-icon", Svg { icon: Icon::Volume, size: 20 } }
                span { class: "topbar-title", "{name}" }
                span { class: "e2ee-badge", Svg { icon: Icon::ShieldCheck, size: 14 } {t("call.e2ee")} }
                span { class: "topbar-spacer" }
                span { class: "muted small", "{count} {t(\"call.in_call\")}" }
                // Voice channels have a chat of their own.
                if state.channel_kind(channel) == Some(sivryn_client_core::protocol::model::ChannelKind::Voice) {
                    IconButton { icon: Icon::Message, size: 18, title: t("channels.open_chat"), onclick: move |_| state.voice_chat.set(true) }
                }
            }
            }
            div { class: if focus.is_some() { "stage focus" } else { "stage" },
                if let Some(feed) = big_screen.clone() {
                    div { class: "stage-main",
                        VideoTile { key: "{feed.track_sid}", feed, large: true }
                    }
                }
                if let Some(p) = big_user.clone() {
                    div { class: "stage-main",
                        ParticipantTile {
                            key: "{p.identity}",
                            camera: cameras.iter().find(|f| f.participant == p.identity).cloned(),
                            speaking: speakers.contains(&p.identity),
                            participant: p,
                            large: true,
                        }
                    }
                }
                if focus.is_some() {
                    div { class: "strip-toggle",
                        onclick: move |_| {
                            if let Some(c) = state.call.write().as_mut() {
                                c.strip_hidden = !c.strip_hidden;
                            }
                        },
                        if strip_hidden { Svg { icon: Icon::ChevronDown, size: 14 } } else { Svg { icon: Icon::ChevronRight, size: 14 } }
                        if strip_hidden { {t("call.show_members")} } else { {t("call.hide_members")} }
                    }
                }
                if focus.is_none() || !strip_hidden {
                    div { class: "stage-grid",
                        for feed in other_screens {
                            VideoTile { key: "{feed.track_sid}", feed: feed.clone(), large: false }
                        }
                        for p in other_people {
                            ParticipantTile {
                                key: "{p.identity}",
                                camera: cameras.iter().find(|f| f.participant == p.identity).cloned(),
                                speaking: speakers.contains(&p.identity),
                                participant: p,
                                large: false,
                            }
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
fn ParticipantTile(participant: Participant, camera: Option<VideoFeed>, speaking: bool, large: bool) -> Element {
    let state = use_context::<AppState>();
    let call_channel = state.call.read().as_ref().map(|c| c.channel);
    let id: Option<sivryn_client_core::protocol::UserId> = participant.identity.parse().ok();
    let class = match (large, speaking) {
        (true, true) => "tile participant large speaking",
        (true, false) => "tile participant large",
        (false, true) => "tile participant speaking",
        (false, false) => "tile participant",
    };
    let (identity, sharing) = (participant.identity.clone(), participant.sharing);
    // The name on this server (nickname or display name).
    let name = id
        .map(|id| state.display_name_in(call_channel.and_then(|c| state.guild_of(c)), id))
        .unwrap_or(participant.name.clone());
    let open_menu = move |e: MouseEvent| {
        let Some(user) = id else { return };
        let p = e.client_coordinates();
        let guild = call_channel.and_then(|c| state.guild_of(c));
        let mut items = super::menu::user_items(state, user, guild, call_channel);
        if sharing && Some(user) != state.me() {
            items.insert(
                items.iter().position(|i| matches!(i, super::MenuItem::Check { .. })).unwrap_or(items.len()),
                super::MenuItem::Volume { label: t("call.stream_volume").into(), key: screen_volume_key(&identity) },
            );
        }
        super::menu::open(state, p.x, p.y, items);
    };

    let focus_key = Focus::User(participant.identity.clone());
    rsx! {
        div { class: "{class}",
            // Left click shows this person large (again: back to the grid).
            onclick: move |_| toggle_focus(state, focus_key.clone()),
            onmousedown: move |e| {
                if super::menu::right_click(&e).is_some() {
                    open_menu(e);
                }
            },
            match camera {
                Some(feed) => rsx! { VideoCanvas { key: "{feed.track_sid}", feed } },
                None => rsx! {
                    div { class: "tile-avatar",
                        Avatar { name: name.clone(), id, size: if large { 128 } else { 76 }, speaking }
                    }
                },
            }
            div { class: "name-tag",
                if participant.muted {
                    span { class: "tag-muted", Svg { icon: Icon::MicOff, size: 13 } }
                }
                span { "{name}" }
                if participant.sharing {
                    span { class: "live-badge", "LIVE" }
                }
            }
        }
    }
}

/// Shows a tile large, or goes back to the grid when it already is.
fn toggle_focus(mut state: AppState, key: Focus) {
    if let Some(c) = state.call.write().as_mut() {
        let current = c.focus.clone().or_else(|| {
            c.feeds
                .iter()
                .find(|f| f.source == TrackSource::Screenshare)
                .filter(|_| c.auto_focus)
                .map(|f| Focus::Screen(f.track_sid.clone()))
        });
        if current.as_ref() == Some(&key) {
            c.focus = None;
            c.auto_focus = false;
        } else {
            c.focus = Some(key);
        }
    }
}

#[component]
fn VideoTile(feed: VideoFeed, large: bool) -> Element {
    let state = use_context::<AppState>();
    let guild = state.call.read().as_ref().and_then(|c| state.guild_of(c.channel));
    let name = feed.participant.parse().map(|id| state.display_name_in(guild, id)).unwrap_or(feed.participant.clone());
    let sid = feed.track_sid.clone();
    rsx! {
        div {
            class: if large { "tile screen large" } else { "tile screen" },
            onclick: move |_| toggle_focus(state, Focus::Screen(sid.clone())),
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
    // Live thumbnails while the picker is open (dropped with it).
    let previews = use_hook({
        let sources = sources.clone();
        move || Rc::new(SourcePreviews::start(&sources))
    });
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
        let limits = video.screen_limits(&c.join.screen_share);
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

    let windows_tab = video.share_windows_tab;
    let kind = if windows_tab { SourceKind::Window } else { SourceKind::Screen };
    let shown: Vec<ScreenSource> = sources.iter().filter(|s| s.kind == kind).cloned().collect();
    let quality_label = video.screen_label();

    rsx! {
        div { class: "overlay", onclick: move |_| state.modal.set(None),
            div { class: "dialog share-dialog", onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { {t("share.title")} }
                    IconButton { icon: Icon::X, size: 18, onclick: move |_| state.modal.set(None) }
                }
                div { class: "share-tabs",
                    div { class: if !windows_tab { "tab active" } else { "tab" },
                        onclick: move |_| state.update_settings(|s| s.video.share_windows_tab = false),
                        Svg { icon: Icon::Monitor, size: 16 }
                        {t("share.screens")}
                    }
                    div { class: if windows_tab { "tab active" } else { "tab" },
                        onclick: move |_| state.update_settings(|s| s.video.share_windows_tab = true),
                        Svg { icon: Icon::Maximize, size: 16 }
                        {t("share.windows")}
                    }
                }
                div { class: "share-grid",
                    if shown.is_empty() {
                        div { class: "muted", {t("share.none")} }
                    }
                    for source in shown {
                        {
                            let thumb = previews.frames.get(&source.id).cloned();
                            let s2 = source.clone();
                            rsx! {
                                div {
                                    key: "{source.id}",
                                    class: "share-card",
                                    onclick: move |_| pick.call(s2.clone()),
                                    div { class: "share-thumb",
                                        if let Some(frames) = thumb {
                                            FrameCanvas { frames: Frames(frames) }
                                        }
                                    }
                                    div { class: "share-card-title",
                                        if source.kind == SourceKind::Screen {
                                            Svg { icon: Icon::Monitor, size: 14 }
                                        } else {
                                            Svg { icon: Icon::Maximize, size: 14 }
                                        }
                                        span { class: "source-title", "{source.title}" }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "share-options",
                    div { class: "share-option",
                        span { class: "field-label", {t("share.resolution")} }
                        div { class: "segmented",
                            for r in ScreenResolution::ALL {
                                div {
                                    key: "{r:?}",
                                    class: if video.screen_resolution == r { "seg active" } else { "seg" },
                                    onclick: move |_| state.update_settings(|s| s.video.screen_resolution = r),
                                    "{r.label()}"
                                }
                            }
                        }
                    }
                    div { class: "share-option",
                        span { class: "field-label", {t("share.fps")} }
                        div { class: "segmented",
                            for fps in SCREEN_FPS {
                                div {
                                    key: "{fps}",
                                    class: if video.screen_fps == fps { "seg active" } else { "seg" },
                                    onclick: move |_| state.update_settings(|s| s.video.screen_fps = fps),
                                    "{fps} FPS"
                                }
                            }
                        }
                    }
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
                    div { class: "share-option inline",
                        Toggle { on: video.screen_audio, onchange: move |v| state.update_settings(|s| s.video.screen_audio = v) }
                        span { {t("share.audio")} }
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

/// Starts or joins the call in a channel (voice channel or DM), optionally
/// with the camera on.
pub fn start(state: AppState, channel: ChannelId, camera: bool) {
    if state.call.peek().as_ref().is_some_and(|c| c.channel == channel) {
        if camera && !state.call.peek().as_ref().is_some_and(|c| c.camera_on) {
            toggle_camera(state);
        }
        return;
    }
    let engine = super::use_services_engine();
    spawn_forever(async move {
        match connect(state, channel, engine.clone()).await {
            Ok(()) => {
                engine.play(Sound::Join);
                if camera {
                    toggle_camera(state);
                }
            }
            Err(e) => state.report(Err(e)),
        }
    });
}

/// The call area above a DM's messages: the call itself while connected,
/// otherwise a banner when others are in it.
#[component]
pub fn DmCall(channel: ChannelId) -> Element {
    let state = use_context::<AppState>();
    let in_call = state.call.read().as_ref().is_some_and(|c| c.channel == channel);
    let mut others = use_signal(Vec::<sivryn_client_core::protocol::UserId>::new);
    use_future(move || async move {
        loop {
            if let Some(s) = state.session.cloned()
                && !state.call.peek().as_ref().is_some_and(|c| c.channel == channel)
            {
                let now: Vec<_> = match s.api.channel_voice_state(channel).await {
                    Ok(v) => v.participants.into_iter().map(|p| p.user_id).collect(),
                    Err(_) => Vec::new(),
                };
                if *others.peek() != now {
                    others.set(now);
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
    if in_call {
        return rsx! { div { class: "dm-call", CallView { compact: true } } };
    }
    let present = others.read().clone();
    if present.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "dm-call-banner",
            div { class: "join-present",
                for u in present.iter() {
                    Avatar { key: "{u}", name: state.username(*u), id: Some(*u), size: 28 }
                }
            }
            span { {t("call.ongoing")} }
            span { class: "topbar-spacer" }
            button { class: "button primary small", onclick: move |_| start(state, channel, false),
                Svg { icon: Icon::Phone, size: 15 }
                {t("call.join")}
            }
        }
    }
}

/// Rings until the call is answered, declined or 30 seconds have passed.
pub fn ring(mut state: AppState) {
    static RINGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if RINGING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let engine = super::use_services_engine();
    spawn_forever(async move {
        let started = std::time::Instant::now();
        let first = *state.ringing.peek();
        while state.ringing.peek().is_some() && *state.ringing.peek() == first {
            if started.elapsed() > std::time::Duration::from_secs(30) {
                state.ringing.set(None);
                break;
            }
            engine.play(Sound::Ring);
            tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
        }
        RINGING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

#[component]
pub fn IncomingCall() -> Element {
    let mut state = use_context::<AppState>();
    let Some((channel, caller)) = *state.ringing.read() else { return rsx! {} };
    let name = state.username(caller);
    let group = state
        .dm(channel)
        .filter(|d| d.channel.kind == sivryn_client_core::protocol::model::ChannelKind::Group)
        .map(|d| state.dm_title(&d));
    let subtitle = match group {
        Some(group) => format!("{} {group}", t("call.incoming_group")),
        None => t("call.incoming").to_string(),
    };

    rsx! {
        div { class: "incoming-call",
            Avatar { name: name.clone(), id: Some(caller), size: 72 }
            div { class: "incoming-name", "{name}" }
            div { class: "muted small", "{subtitle}" }
            div { class: "incoming-actions",
                IconButton { icon: Icon::PhoneOff, class: "control danger", title: t("call.decline"), onclick: move |_| state.ringing.set(None) }
                IconButton {
                    icon: Icon::Video,
                    class: "control accept",
                    title: t("call.accept_video"),
                    onclick: move |_| {
                        state.ringing.set(None);
                        state.open_channel(channel);
                        start(state, channel, true);
                    },
                }
                IconButton {
                    icon: Icon::Phone,
                    class: "control accept",
                    title: t("call.accept"),
                    onclick: move |_| {
                        state.ringing.set(None);
                        state.open_channel(channel);
                        start(state, channel, false);
                    },
                }
            }
        }
    }
}

/// Milliseconds since the last keyboard or mouse input anywhere.
#[cfg(windows)]
fn idle_ms() -> u64 {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: plain query into a correctly sized struct.
    if unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        // SAFETY: no arguments.
        u64::from(unsafe { GetTickCount() }.wrapping_sub(info.dwTime))
    } else {
        0
    }
}

#[cfg(not(windows))]
fn idle_ms() -> u64 {
    0
}

/// Moves you to the guild's AFK channel after its timeout without input and
/// without speaking.
pub async fn afk_watch(state: AppState) {
    let engine = super::use_services_engine();
    let mut last_spoke = std::time::Instant::now();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if engine.transmitting() {
            last_spoke = std::time::Instant::now();
        }
        let Some(channel) = state.call.peek().as_ref().map(|c| c.channel) else { continue };
        let Some(guild) = state.guild_of(channel).and_then(|g| state.guild(g)) else { continue };
        let Some(afk) = guild.afk_channel_id.filter(|afk| *afk != channel) else { continue };
        let timeout = u64::from(guild.afk_timeout_minutes.max(1)) * 60_000;
        if idle_ms() >= timeout && last_spoke.elapsed().as_millis() as u64 >= timeout {
            tracing::info!("idle, moving to the AFK channel");
            match connect(state, afk, engine.clone()).await {
                Ok(()) => state.toast(t("call.moved_afk"), false),
                Err(e) => state.report(Err(e)),
            }
        }
    }
}
