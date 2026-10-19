//! Main layout: server rail, channel sidebar with voice presence, the user
//! panel, the content area and dialogs.

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::ChannelId;
use sivryn_client_core::protocol::model::ChannelKind;

use super::icons::{Icon, Logo, Svg};
use super::widgets::{Avatar, IconButton};
use super::{AppState, Connection, Modal, SettingsPage, call, chat_view, settings_view};
use crate::i18n::t;

#[component]
pub fn Shell() -> Element {
    let state = use_context::<AppState>();
    let channel = *state.channel.read();
    let in_call_here = state.call.read().as_ref().is_some_and(|c| Some(c.channel) == channel);

    rsx! {
        div { class: "app",
            GuildRail {}
            Sidebar {}
            main { class: "main",
                if *state.connection.read() == Connection::Offline {
                    div { class: "offline-bar", {t("shell.offline")} }
                }
                match channel.and_then(|c| state.channel_kind(c)) {
                    Some(ChannelKind::Text) => rsx! { chat_view::ChatView { channel: channel.unwrap() } },
                    Some(ChannelKind::Voice) if in_call_here => rsx! { call::CallView {} },
                    Some(ChannelKind::Voice) => rsx! { call::JoinPrompt { channel: channel.unwrap() } },
                    None => rsx! { Welcome {} },
                }
            }
            if state.modal.read().is_some() {
                ModalView {}
            }
            if state.settings_page.read().is_some() {
                settings_view::SettingsView {}
            }
        }
    }
}

#[component]
fn Welcome() -> Element {
    let mut state = use_context::<AppState>();
    let has_guilds = !state.guilds.read().is_empty();
    rsx! {
        div { class: "empty-state",
            Logo { size: 64 }
            if has_guilds {
                h2 { {t("shell.pick_channel")} }
                p { class: "muted", {t("shell.pick_channel_hint")} }
            } else {
                h2 { {t("shell.no_servers")} }
                p { class: "muted", {t("shell.no_servers_hint")} }
                div { class: "row",
                    button { class: "button primary", onclick: move |_| state.modal.set(Some(Modal::CreateGuild)), {t("shell.create_server")} }
                    button { class: "button", onclick: move |_| state.modal.set(Some(Modal::JoinGuild)), {t("shell.join_server")} }
                }
            }
        }
    }
}

#[component]
fn GuildRail() -> Element {
    let mut state = use_context::<AppState>();
    let selected = *state.guild.read();

    rsx! {
        nav { class: "rail",
            div { class: "rail-logo", Logo { size: 40 } }
            div { class: "rail-sep" }
            for g in state.guilds.read().iter() {
                div {
                    key: "{g.guild.id}",
                    class: "rail-entry",
                    div { class: if Some(g.guild.id) == selected { "rail-pill active" } else { "rail-pill" } }
                    div {
                        class: if Some(g.guild.id) == selected { "rail-item active" } else { "rail-item" },
                        title: "{g.guild.name}",
                        onclick: {
                            let id = g.guild.id;
                            move |_| {
                                if *state.guild.peek() != Some(id) {
                                    state.guild.set(Some(id));
                                    state.channel.set(None);
                                    state.update_settings(|s| s.last_guild = Some(id));
                                    spawn_forever(state.refresh_voice());
                                }
                            }
                        },
                        "{initials(&g.guild.name)}"
                    }
                }
            }
            div { class: "rail-entry",
                div { class: "rail-pill" }
                div { class: "rail-item add", title: t("shell.create_server"), onclick: move |_| state.modal.set(Some(Modal::CreateGuild)),
                    Svg { icon: Icon::Plus, size: 22 }
                }
            }
            div { class: "rail-entry",
                div { class: "rail-pill" }
                div { class: "rail-item add", title: t("shell.join_server"), onclick: move |_| state.modal.set(Some(Modal::JoinGuild)),
                    Svg { icon: Icon::LogIn, size: 20 }
                }
            }
        }
    }
}

#[component]
fn Sidebar() -> Element {
    let mut state = use_context::<AppState>();
    let guilds = state.guilds.read();
    let Some(guild) = guilds.iter().find(|g| Some(g.guild.id) == *state.guild.read()) else {
        return rsx! {
            aside { class: "sidebar",
                div { class: "sidebar-header", "Sivryn" }
                div { class: "channel-list" }
                VoicePanel {}
                UserPanel {}
            }
        };
    };
    let selected = *state.channel.read();
    let unread = state.unread.read();
    let voice = state.voice.read();
    let call = state.call.read();
    let text: Vec<_> = guild.channels.iter().filter(|c| c.kind == ChannelKind::Text).cloned().collect();
    let voice_channels: Vec<_> = guild.channels.iter().filter(|c| c.kind == ChannelKind::Voice).cloned().collect();
    let mut open = move |id: ChannelId| {
        state.channel.set(Some(id));
        state.unread.write().remove(&id);
    };

    rsx! {
        aside { class: "sidebar",
            div { class: "sidebar-header",
                span { class: "guild-title", "{guild.guild.name}" }
                IconButton { icon: Icon::UserPlus, size: 18, title: t("shell.invite"), onclick: move |_| state.modal.set(Some(Modal::Invite)) }
            }
            div { class: "channel-list",
                div { class: "category", {t("shell.text_channels")} }
                for c in text {
                    div {
                        key: "{c.id}",
                        class: if Some(c.id) == selected { "channel active" } else if unread.contains(&c.id) { "channel unread" } else { "channel" },
                        onclick: move |_| open(c.id),
                        Svg { icon: Icon::Hash, size: 18 }
                        span { class: "channel-name", "{c.name}" }
                        if unread.contains(&c.id) && Some(c.id) != selected {
                            div { class: "unread-dot" }
                        }
                    }
                }
                div { class: "category", {t("shell.voice_channels")} }
                for c in voice_channels {
                    div { key: "{c.id}",
                        div {
                            class: if Some(c.id) == selected { "channel active" } else { "channel" },
                            onclick: move |_| open(c.id),
                            Svg { icon: Icon::Volume, size: 18 }
                            span { class: "channel-name", "{c.name}" }
                        }
                        div { class: "voice-members",
                            // While connected, the live room is more current than the server's list.
                            if let Some(live) = call.as_ref().filter(|call| call.channel == c.id) {
                                for p in live.participants.iter() {
                                    VoiceMember {
                                        key: "{p.identity}",
                                        name: p.name.clone(),
                                        id: p.identity.parse().ok(),
                                        muted: p.muted,
                                        sharing: p.sharing,
                                        speaking: live.speakers.contains(&p.identity),
                                    }
                                }
                            } else {
                                for p in voice.get(&c.id).into_iter().flatten() {
                                    VoiceMember {
                                        key: "{p.user_id}",
                                        name: state.username(p.user_id),
                                        id: Some(p.user_id),
                                        muted: p.muted,
                                        sharing: p.screen_sharing,
                                        speaking: false,
                                    }
                                }
                            }
                        }
                    }
                }
            }
            VoicePanel {}
            UserPanel {}
        }
    }
}

#[component]
fn VoiceMember(
    name: String,
    id: Option<sivryn_client_core::protocol::UserId>,
    muted: bool,
    sharing: bool,
    speaking: bool,
) -> Element {
    rsx! {
        div { class: "voice-member",
            Avatar { name: name.clone(), id, size: 22, speaking }
            span { class: "voice-member-name", "{name}" }
            if sharing {
                span { class: "live-badge", "LIVE" }
            }
            if muted {
                span { class: "member-muted", Svg { icon: Icon::MicOff, size: 14 } }
            }
        }
    }
}

/// Shown while connected to voice: where you are, plus quick controls.
#[component]
fn VoicePanel() -> Element {
    let mut state = use_context::<AppState>();
    let call = state.call.read();
    let Some(c) = call.as_ref() else { return rsx! {} };
    let channel = c.channel;
    let name = state.channel_name(channel);
    let (sharing, camera_on) = (c.sharing, c.camera_on);
    drop(call);

    rsx! {
        div { class: "voice-panel",
            div { class: "voice-status",
                div {
                    class: "voice-connected",
                    onclick: move |_| state.channel.set(Some(channel)),
                    span { class: "status-dot online" }
                    div {
                        div { class: "voice-title", {t("call.connected")} }
                        div { class: "voice-sub", "{name}" }
                    }
                }
                IconButton { icon: Icon::PhoneOff, class: "danger-ghost", title: t("call.leave"), onclick: move |_| call::hang_up(state) }
            }
            div { class: "voice-actions",
                if camera_on {
                    IconButton { icon: Icon::Video, class: "active", size: 18, title: t("call.camera_off"), onclick: move |_| call::toggle_camera(state) }
                } else {
                    IconButton { icon: Icon::VideoOff, size: 18, title: t("call.camera_on"), onclick: move |_| call::toggle_camera(state) }
                }
                if sharing {
                    IconButton { icon: Icon::MonitorOff, class: "active", size: 18, title: t("call.stop_share"), onclick: move |_| call::stop_share(state) }
                } else {
                    IconButton { icon: Icon::MonitorUp, size: 18, title: t("call.share"), onclick: move |_| state.modal.set(Some(Modal::PickScreen)) }
                }
            }
        }
    }
}

#[component]
fn UserPanel() -> Element {
    let mut state = use_context::<AppState>();
    let services = use_context::<super::Services>();
    // Poll the engine so the avatar ring follows your own voice.
    let mut speaking = use_signal(|| false);
    use_future(move || {
        let engine = services.engine.clone();
        async move {
            loop {
                let now = engine.transmitting();
                if *speaking.peek() != now {
                    speaking.set(now);
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    });
    let session = state.session.read();
    let Some(s) = session.as_ref() else { return rsx! {} };
    let (name, id) = (s.user.display_name.clone().unwrap_or_else(|| s.user.username.clone()), s.user.id);
    drop(session);
    let muted = *state.muted.read();
    let deafened = *state.deafened.read();
    let in_call = state.call.read().is_some();
    let connection = *state.connection.read();
    let status = match connection {
        Connection::Online => t("shell.online"),
        Connection::Connecting => t("shell.connecting"),
        Connection::Offline => t("shell.offline_short"),
    };
    let dot = match connection {
        Connection::Online => "status-dot online",
        Connection::Connecting => "status-dot connecting",
        Connection::Offline => "status-dot offline",
    };

    rsx! {
        div { class: "user-panel",
            div { class: "user-info",
                Avatar { name: name.clone(), id: Some(id), size: 34, speaking: in_call && *speaking.read() }
                div { class: "user-text",
                    div { class: "user-name", "{name}" }
                    div { class: "user-status", span { class: "{dot}" } "{status}" }
                }
            }
            div { class: "user-actions",
                if muted || deafened {
                    IconButton { icon: Icon::MicOff, class: "alert", size: 18, title: t("call.unmute"), onclick: move |_| state.set_muted(false) }
                } else {
                    IconButton { icon: Icon::Mic, size: 18, title: t("call.mute"), onclick: move |_| state.set_muted(true) }
                }
                if deafened {
                    IconButton { icon: Icon::HeadphonesOff, class: "alert", size: 18, title: t("call.undeafen"), onclick: move |_| state.set_deafened(false) }
                } else {
                    IconButton { icon: Icon::Headphones, size: 18, title: t("call.deafen"), onclick: move |_| state.set_deafened(true) }
                }
                IconButton { icon: Icon::Settings, size: 18, title: t("settings.title"), onclick: move |_| state.settings_page.set(Some(SettingsPage::Voice)) }
            }
        }
    }
}

#[component]
fn ModalView() -> Element {
    let state = use_context::<AppState>();
    // One component per dialog, so each gets its own hooks and state.
    match *state.modal.read() {
        Some(Modal::PickScreen) => rsx! { call::ScreenPicker {} },
        Some(Modal::Invite) => rsx! { InviteDialog {} },
        Some(modal) => rsx! { GuildDialog { key: "{modal as u8}", modal } },
        None => rsx! {},
    }
}

#[component]
fn InviteDialog() -> Element {
    let mut state = use_context::<AppState>();
    let mut invite = use_signal(|| None::<String>);
    let mut error = use_signal(|| None::<String>);
    // Create the invite as soon as the dialog opens.
    use_hook(move || {
        let (Some(s), Some(guild)) = (state.session.cloned(), *state.guild.peek()) else { return };
        spawn(async move {
            match s.api.create_invite(guild).await {
                Ok(code) => invite.set(Some(code)),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });
    rsx! {
        div { class: "overlay", onclick: move |_| state.modal.set(None),
            div { class: "dialog", onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { {t("invite.title")} }
                    IconButton { icon: Icon::X, size: 18, onclick: move |_| state.modal.set(None) }
                }
                p { class: "muted", {t("invite.hint")} }
                div { class: "invite-code",
                    match invite.read().as_ref() {
                        Some(code) => rsx! { span { class: "code", "{code}" } },
                        None => rsx! { span { class: "muted", {t("common.wait")} } },
                    }
                }
                if let Some(err) = error.read().as_ref() {
                    div { class: "form-error", "{err}" }
                }
                div { class: "dialog-actions",
                    button { class: "button primary", onclick: move |_| state.modal.set(None), {t("common.done")} }
                }
            }
        }
    }
}

#[component]
fn GuildDialog(modal: Modal) -> Element {
    let mut state = use_context::<AppState>();
    let mut text = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let (title, hint, placeholder, action) = match modal {
        Modal::CreateGuild => (t("create.title"), t("create.hint"), t("create.placeholder"), t("create.action")),
        _ => (t("join.title"), t("join.hint"), t("join.placeholder"), t("join.action")),
    };

    // The dialog stays open until the request is done: a task spawned here is
    // cancelled as soon as this component unmounts.
    let mut confirm = move || {
        let Some(s) = state.session.cloned() else { return };
        let value = text.read().trim().to_owned();
        if value.is_empty() || *busy.read() {
            return;
        }
        busy.set(true);
        error.set(None);
        spawn(async move {
            let result = match modal {
                Modal::CreateGuild => s.api.create_guild(&value).await,
                _ => s.api.accept_invite(&value).await,
            };
            busy.set(false);
            match result {
                Ok(g) => {
                    let id = g.guild.id;
                    state.guild.set(Some(id));
                    state.channel.set(g.channels.iter().find(|c| c.kind == ChannelKind::Text).map(|c| c.id));
                    state.update_settings(|s| s.last_guild = Some(id));
                    state.modal.set(None);
                    // Keep refreshing even though this dialog is gone now.
                    spawn_forever(async move { state.report(state.refresh().await) });
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    rsx! {
        div { class: "overlay", onclick: move |_| state.modal.set(None),
            div { class: "dialog", onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { "{title}" }
                    IconButton { icon: Icon::X, size: 18, onclick: move |_| state.modal.set(None) }
                }
                p { class: "muted", "{hint}" }
                input {
                    class: "input",
                    autofocus: "true",
                    value: "{text}",
                    placeholder: "{placeholder}",
                    oninput: move |e| text.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            confirm();
                        } else if e.key() == Key::Escape {
                            state.modal.set(None);
                        }
                    },
                }
                if let Some(err) = error.read().as_ref() {
                    div { class: "form-error", "{err}" }
                }
                div { class: "dialog-actions",
                    button { class: "button ghost", onclick: move |_| state.modal.set(None), {t("common.cancel")} }
                    button { class: if *busy.read() { "button primary busy" } else { "button primary" }, onclick: move |_| confirm(),
                        if *busy.read() { {t("common.wait")} } else { "{action}" }
                    }
                }
            }
        }
    }
}

fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase()
}
