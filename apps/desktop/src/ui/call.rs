//! Voice channel UI: join prompt, video grid, controls and screen picker.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus_native::prelude::*;
use dioxus_native::use_wgpu;
use sivryn_client_core::protocol::ChannelId;
use sivryn_client_core::protocol::media::{ContentHint, VideoCodec};
use sivryn_media::{CallEvent, ScreenShare, ScreenSource, TrackSource, VideoFeed, VoiceCall, list_sources};

use super::{AppState, CallState, Modal};
use crate::chat;
use crate::video_surface::VideoSurface;

#[component]
pub fn JoinPrompt(channel: ChannelId) -> Element {
    let mut state = use_context::<AppState>();
    let mut joining = use_signal(|| false);

    let join = move |_| {
        let Some(s) = state.session.cloned() else { return };
        let Some(guild) = state.guild_of(channel) else { return };
        joining.set(true);
        spawn(async move {
            let result: Result<(), String> = async {
                if let Some(old) = state.call.take() {
                    leave(old).await;
                }
                let join = s.api.join_voice(channel).await.map_err(|e| e.to_string())?;
                let media_key = if join.e2ee {
                    if !chat::ensure_group(&s, guild, channel).await? {
                        return Err("waiting for a member to add this device to the encrypted call".into());
                    }
                    Some(s.mls.borrow().media_key(channel).map_err(|e| e.to_string())?)
                } else {
                    None
                };
                let (mut call, mut events) = VoiceCall::join(&join, media_key).await.map_err(|e| e.to_string())?;
                if let Err(e) = call.enable_microphone().await {
                    state.report(Err(format!("microphone unavailable: {e}")));
                }

                state.call.set(Some(CallState {
                    channel,
                    join,
                    call: Rc::new(RefCell::new(Some(call))),
                    share: Rc::new(RefCell::new(None)),
                    feeds: Vec::new(),
                    speakers: Vec::new(),
                    muted: false,
                    sharing: false,
                }));

                spawn(async move {
                    while let Some(event) = events.recv().await {
                        let mut call = state.call.write();
                        let Some(c) = call.as_mut().filter(|c| c.channel == channel) else { break };
                        match event {
                            CallEvent::VideoAdded(feed) => c.feeds.push(feed),
                            CallEvent::VideoRemoved { track_sid } => c.feeds.retain(|f| f.track_sid != track_sid),
                            CallEvent::SpeakingChanged { speakers } => c.speakers = speakers,
                            CallEvent::Disconnected => {
                                *call = None;
                                break;
                            }
                            CallEvent::ParticipantJoined { .. } | CallEvent::ParticipantLeft { .. } => {}
                        }
                    }
                });
                Ok(())
            }
            .await;
            joining.set(false);
            state.report(result);
        });
    };

    rsx! {
        div { class: "join",
            h2 { "Voice channel" }
            p { class: "muted", "Screen shares up to 1440p60 with AV1 when your GPU supports it." }
            button { class: "primary big", disabled: *joining.read(), onclick: join,
                if *joining.read() { "Connecting…" } else { "Join voice" }
            }
        }
    }
}

async fn leave(call: CallState) {
    let share = call.share.borrow_mut().take();
    if let Some(share) = share {
        share.stop().await;
    }
    let vc = call.call.borrow_mut().take();
    if let Some(vc) = vc {
        vc.leave().await;
    }
}

#[component]
pub fn CallView() -> Element {
    let mut state = use_context::<AppState>();
    let call = state.call.read();
    let Some(c) = call.as_ref() else { return rsx! {} };

    // Screen shares first and large; cameras below.
    let mut feeds = c.feeds.clone();
    feeds.sort_by_key(|f| f.source != TrackSource::Screenshare);
    let muted = c.muted;
    let sharing = c.sharing;

    rsx! {
        div { class: "call",
            div { class: "stage",
                if feeds.is_empty() {
                    div { class: "empty", "Nobody is sharing video yet" }
                }
                for feed in feeds {
                    VideoTile { key: "{feed.track_sid}", feed: feed.clone() }
                }
            }
            div { class: "controls",
                button {
                    onclick: move |_| {
                        if let Some(c) = state.call.write().as_mut() {
                            c.muted = !c.muted;
                            if let Some(vc) = c.call.borrow().as_ref() {
                                vc.set_muted(c.muted);
                            }
                        }
                    },
                    if muted { "Unmute" } else { "Mute" }
                }
                if sharing {
                    button {
                        onclick: move |_| {
                            let share = state.call.write().as_mut().and_then(|c| {
                                c.sharing = false;
                                c.share.borrow_mut().take()
                            });
                            if let Some(share) = share {
                                spawn(share.stop());
                            }
                        },
                        "Stop sharing"
                    }
                } else {
                    button { class: "primary", onclick: move |_| state.modal.set(Some(Modal::PickScreen)), "Share screen" }
                }
                button {
                    class: "danger",
                    onclick: move |_| {
                        if let Some(call) = state.call.take() {
                            spawn(leave(call));
                        }
                    },
                    "Leave"
                }
            }
        }
    }
}

#[component]
fn VideoTile(feed: VideoFeed) -> Element {
    let state = use_context::<AppState>();
    let frames = feed.frames.clone();
    let paint_source = use_wgpu(move || VideoSurface::new(frames));
    let name = feed.participant.parse().map(|id| state.username(id)).unwrap_or(feed.participant.clone());
    let class = if feed.source == TrackSource::Screenshare { "tile screen" } else { "tile" };

    rsx! {
        div { class: "{class}",
            canvas { class: "video", "src": "{paint_source}" }
            span { class: "label", "{name}" }
        }
    }
}

#[component]
pub fn ScreenPicker() -> Element {
    let mut state = use_context::<AppState>();
    let sources = use_hook(list_sources);
    let mut hint = use_signal(|| ContentHint::Detail);

    let mut pick = move |source: ScreenSource| {
        state.modal.set(None);
        let Some(c) = state.call.cloned() else { return };
        let Some(room) = c.call.borrow().as_ref().map(|vc| vc.room.clone()) else { return };
        let limits = c.join.screen_share.clone();
        let content_hint = *hint.read();
        spawn(async move {
            // TODO: collect decoder support from participants instead of assuming it.
            let viewers = [VideoCodec::Av1, VideoCodec::H264];
            match ScreenShare::start(room, &source, &limits, content_hint, &viewers).await {
                Ok(share) => {
                    tracing::info!(codec = ?share.codec, "screen share started");
                    *c.share.borrow_mut() = Some(share);
                    if let Some(c) = state.call.write().as_mut() {
                        c.sharing = true;
                    }
                }
                Err(e) => state.report(Err(e.to_string())),
            }
        });
    };

    rsx! {
        div { class: "overlay",
            div { class: "card wide",
                h2 { "Share your screen" }
                div { class: "row",
                    button {
                        class: if *hint.read() == ContentHint::Detail { "primary" } else { "" },
                        onclick: move |_| hint.set(ContentHint::Detail),
                        "Sharp text"
                    }
                    button {
                        class: if *hint.read() == ContentHint::Motion { "primary" } else { "" },
                        onclick: move |_| hint.set(ContentHint::Motion),
                        "Smooth motion"
                    }
                }
                div { class: "sources",
                    for source in sources.iter().cloned() {
                        div {
                            key: "{source.id}",
                            class: "source",
                            onclick: {
                                let source = source.clone();
                                move |_| pick(source.clone())
                            },
                            "{source.title}"
                        }
                    }
                }
                button { onclick: move |_| state.modal.set(None), "Cancel" }
            }
        }
    }
}
