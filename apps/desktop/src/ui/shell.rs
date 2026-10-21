//! Main layout: server rail, sidebar (guild channels or friends and DMs),
//! the user panel, the content area and dialogs.

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::model::{ChannelKind, UserStatus};
use sivryn_client_core::protocol::{ChannelId, GuildId, UserId};

use super::guild_settings::{GuildIcon, GuildSettings};
use super::icons::{Icon, Logo, Svg};
use super::widgets::{Avatar, IconButton, StatusDot};
use super::{AppState, Connection, Modal, SettingsPage, call, channels, chat_view, home, menu, profile, settings_view};
use crate::i18n::t;

#[component]
pub fn Shell() -> Element {
    let state = use_context::<AppState>();
    let channel = *state.channel.read();
    let home = *state.home.read();
    let drag = *state.drag_over.read();

    rsx! {
        div { class: "app",
            onmousemove: move |e| {
                use dioxus_native::prelude::dioxus_elements::input_data::MouseButton;
                super::widgets::slider_pointer_move(e.client_coordinates().x, e.held_buttons().contains(MouseButton::Primary));
                super::widgets::hover_root_move();
                super::resize::on_move(state, &e);
                super::dnd::on_move(state, &e);
            },
            onmouseup: move |_| {
                super::widgets::slider_pointer_up();
                super::resize::finish(state);
                super::dnd::finish(state);
            },
            GuildRail {}
            div { class: "sidebar-wrap", style: "width: {super::resize::width(&state, super::resize::Pane::Sidebar)}px;",
                Sidebar {}
                super::resize::ResizeHandle { pane: super::resize::Pane::Sidebar }
            }
            main { class: "main",
                onmousemove: move |_| super::dnd::clear_target(state),
                if *state.connection.read() == Connection::Offline {
                    div { class: "offline-bar", {t("shell.offline")} }
                }
                match channel.and_then(|c| state.channel_kind(c)) {
                    Some(ChannelKind::Voice) => rsx! { call::VoiceArea { channel: channel.unwrap() } },
                    Some(_) => rsx! { chat_view::ChatView { key: "{channel.unwrap()}", channel: channel.unwrap() } },
                    None if home => rsx! { home::FriendsPage {} },
                    None => rsx! { Welcome {} },
                }
                if drag {
                    div { class: "drop-overlay",
                        div { class: "drop-card",
                            Svg { icon: Icon::Upload, size: 40 }
                            div { class: "drop-title", {t("chat.drop_here")} }
                            div { class: "muted", {t("chat.drop_hint")} }
                        }
                    }
                }
            }
            if state.modal.read().is_some() {
                ModalView {}
            }
            if state.settings_page.read().is_some() {
                settings_view::SettingsView {}
            }
            // Above the settings and dialogs it is opened from.
            super::crop::CropDialog {}
            // Last, so it's painted above everything.
            super::dnd::DragGhost {}
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

pub(super) fn select_guild(mut state: AppState, id: GuildId) {
    let switching = *state.home.peek() || *state.guild.peek() != Some(id);
    state.home.set(false);
    if switching {
        state.guild.set(Some(id));
        // Back to the guild's first text channel, like other chat apps.
        let first = state
            .guilds
            .peek()
            .iter()
            .find(|g| g.guild.id == id)
            .and_then(|g| g.channels.iter().find(|c| c.kind == ChannelKind::Text).map(|c| c.id));
        state.channel.set(first);
        state.voice_chat.set(false);
        if let Some(c) = first {
            state.mark_read(c);
        }
        state.update_settings(|s| s.last_guild = Some(id));
        spawn_forever(state.refresh_voice());
    }
}

/// "99+" style counter text.
fn count_label(n: u32) -> String {
    if n > 99 { "99+".into() } else { n.to_string() }
}

#[component]
fn GuildRail() -> Element {
    let mut state = use_context::<AppState>();
    let home = *state.home.read();
    let selected = if home { None } else { *state.guild.read() };
    let me = state.me();
    // DMs with unread messages appear under the logo, like elsewhere.
    let unread_dms: Vec<(ChannelId, String, Option<UserId>, u32)> = state
        .dms
        .read()
        .iter()
        .filter_map(|d| {
            let n = state.unread_count(d.channel.id);
            (n > 0).then(|| {
                let partner = d.members.iter().find(|u| Some(u.id) != me).map(|u| u.id);
                (d.channel.id, state.dm_title(d), partner, n)
            })
        })
        .collect();
    let home_count = state.home_unread();

    rsx! {
        nav { class: "rail",
            div { class: "rail-entry",
                div { class: if home { "rail-pill active" } else { "rail-pill" } }
                div { class: if home { "rail-logo active" } else { "rail-logo" }, title: t("home.title"), onclick: move |_| state.open_home(),
                    Logo { size: 48 }
                    if home_count > 0 {
                        span { class: "rail-badge", "{count_label(home_count)}" }
                    }
                }
            }
            for (id, title, partner, n) in unread_dms {
                div { key: "{id}", class: "rail-entry",
                    div { class: "rail-pill unread" }
                    div { class: "rail-dm", title: "{title}", onclick: move |_| state.open_channel(id),
                        Avatar { name: title.clone(), id: partner, size: 48 }
                        span { class: "rail-badge", "{count_label(n)}" }
                    }
                }
            }
            div { class: "rail-sep" }
            for g in state.guilds.read().iter() {
                {
                    let id = g.guild.id;
                    let active = Some(id) == selected;
                    let unread = state.guild_unread(id);
                    rsx! {
                        div { key: "{id}", class: "rail-entry",
                            div { class: if active { "rail-pill active" } else if unread > 0 { "rail-pill unread" } else { "rail-pill" } }
                            div { class: "rail-icon-wrap",
                                div {
                                    class: if active { "rail-item active" } else { "rail-item" },
                                    title: "{g.guild.name}",
                                    onclick: move |_| select_guild(state, id),
                                    onmousedown: move |e| {
                                        if let Some((x, y)) = menu::right_click(&e) {
                                            menu::open(state, x, y, menu::guild_items(state, id));
                                        }
                                    },
                                    GuildIcon { name: g.guild.name.clone(), icon: g.guild.icon_id, size: 48 }
                                }
                                if unread > 0 {
                                    span { class: "rail-badge", "{count_label(unread)}" }
                                }
                            }
                        }
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
    let state = use_context::<AppState>();
    if *state.home.read() {
        return rsx! {
            aside { class: "sidebar",
                div { class: "sidebar-header", span { class: "guild-title", {t("home.title")} } }
                home::HomeSidebar {}
                VoicePanel {}
                UserPanel {}
            }
        };
    }
    let guild = state.guilds.read().iter().find(|g| Some(g.guild.id) == *state.guild.read()).cloned();
    let Some(guild) = guild else {
        return rsx! {
            aside { class: "sidebar",
                div { class: "sidebar-header", "Sivryn" }
                div { class: "channel-list" }
                VoicePanel {}
                UserPanel {}
            }
        };
    };
    rsx! {
        aside { class: "sidebar",
            channels::GuildHeader { key: "{guild.guild.id}", guild: guild.guild.id, name: guild.guild.name.clone() }
            channels::ChannelList { guild: guild.clone() }
            VoicePanel {}
            UserPanel {}
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
    let id = s.user.id;
    drop(session);
    let name = state.username(id);
    let my_status = *state.my_status.read();
    let muted = *state.muted.read();
    let deafened = *state.deafened.read();
    let in_call = state.call.read().is_some();
    let connection = *state.connection.read();
    let (status, shown) = match connection {
        Connection::Online => (status_label(my_status), Some(my_status)),
        Connection::Connecting => (t("shell.connecting"), Some(UserStatus::Idle)),
        Connection::Offline => (t("shell.offline_short"), None),
    };

    rsx! {
        div { class: "user-panel",
            div {
                class: "user-info",
                title: t("profile.status_menu"),
                onclick: move |e| {
                    let p = e.client_coordinates();
                    menu::open(state, p.x.max(8.0), (p.y - 250.0).max(8.0), menu::status_items(state));
                },
                div { class: "member-avatar",
                    Avatar { name: name.clone(), id: Some(id), size: 34, speaking: in_call && *speaking.read() }
                    StatusDot { status: shown, class: "panel" }
                }
                div { class: "user-text",
                    div { class: "user-name", "{name}" }
                    div { class: "user-status", "{status}" }
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

pub fn status_label(status: UserStatus) -> &'static str {
    match status {
        UserStatus::Online => t("status.online"),
        UserStatus::Idle => t("status.idle"),
        UserStatus::Dnd => t("status.dnd"),
        UserStatus::Invisible => t("status.invisible"),
    }
}

#[component]
fn ModalView() -> Element {
    let state = use_context::<AppState>();
    // One component per dialog, so each gets its own hooks and state.
    match *state.modal.read() {
        Some(Modal::PickScreen) => rsx! { call::ScreenPicker {} },
        Some(Modal::Invite) => rsx! { InviteDialog {} },
        Some(Modal::GuildSettings(guild)) => rsx! { GuildSettings { guild } },
        Some(Modal::NewDm) => rsx! { home::PickPeople { channel: None } },
        Some(Modal::AddToDm(channel)) => rsx! { home::PickPeople { channel: Some(channel) } },
        Some(Modal::ConfirmDelete(channel, message)) => rsx! { chat_view::ConfirmDelete { channel, message } },
        Some(Modal::Profile(user)) => rsx! { profile::ProfileDialog { key: "{user}", user } },
        Some(Modal::ConfirmModeration { guild, user, ban }) => rsx! { ConfirmModeration { guild, user, ban } },
        Some(Modal::CreateChannel(guild, category, kind)) => {
            rsx! { channels::CreateChannelDialog { guild, category, kind } }
        }
        Some(Modal::Category(_, Some(category))) => {
            rsx! { super::place_settings::PlaceSettings { place: super::place_settings::Place::Category(category) } }
        }
        Some(Modal::Category(guild, None)) => rsx! { channels::CategoryDialog { guild, existing: None } },
        Some(Modal::ChannelSettings(channel)) => {
            rsx! { super::place_settings::PlaceSettings { place: super::place_settings::Place::Channel(channel) } }
        }
        Some(Modal::SwitchVoice(channel)) => rsx! { call::SwitchVoiceDialog { channel } },
        Some(Modal::JoinVoice(channel)) => rsx! { call::JoinVoiceDialog { channel } },
        Some(Modal::NotifySettings(guild)) => rsx! { channels::NotifyDialog { guild } },
        Some(modal) => rsx! { GuildDialog { key: "{modal:?}", modal } },
        None => rsx! {},
    }
}

/// Asks before kicking or banning someone; bans take an optional reason.
#[component]
fn ConfirmModeration(guild: GuildId, user: UserId, ban: bool) -> Element {
    let mut state = use_context::<AppState>();
    let mut reason = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let name = state.username(user);
    let server = state.guild(guild).map(|g| g.name).unwrap_or_default();
    let (title, hint, action) = if ban {
        (t("moderation.ban_title"), t("moderation.ban_hint"), t("menu.ban"))
    } else {
        (t("moderation.kick_title"), t("moderation.kick_hint"), t("menu.kick"))
    };
    let confirm = move |_| {
        let Some(s) = state.session.cloned() else { return };
        if *busy.peek() {
            return;
        }
        busy.set(true);
        let reason = reason.peek().trim().to_owned();
        spawn_forever(async move {
            let result = if ban {
                s.api.ban(guild, user, Some(reason).filter(|r| !r.is_empty())).await
            } else {
                s.api.remove_member(guild, user).await
            };
            super::widgets::set_if_alive(busy, false);
            match result {
                Ok(()) => state.modal.set(None),
                Err(e) => state.report(Err(e.to_string())),
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
                p { class: "muted", strong { "{name}" } " – {server}. {hint}" }
                if ban {
                    input {
                        class: "input",
                        value: "{reason}",
                        placeholder: t("moderation.reason"),
                        oninput: move |e| reason.set(e.value()),
                    }
                }
                div { class: "dialog-actions",
                    button { class: "button ghost", onclick: move |_| state.modal.set(None), {t("common.cancel")} }
                    button { class: if *busy.read() { "button danger busy" } else { "button danger" }, onclick: confirm, "{action}" }
                }
            }
        }
    }
}

#[component]
fn InviteDialog() -> Element {
    let mut state = use_context::<AppState>();
    let mut invite = use_signal(|| None::<String>);
    let error = use_signal(|| None::<String>);
    // Create the invite as soon as the dialog opens.
    use_hook(move || {
        let (Some(s), Some(guild)) = (state.session.cloned(), *state.guild.peek()) else { return };
        spawn(async move {
            match s.api.create_invite(guild).await {
                Ok(code) => invite.set(Some(code)),
                Err(e) => super::widgets::set_if_alive(error, Some(e.to_string())),
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
                Modal::CreateGuild => {
                    s.api.create_guild(&value, t("channels.default_text"), t("channels.default_voice")).await
                }
                _ => s.api.accept_invite(&value).await,
            };
            super::widgets::set_if_alive(busy, false);
            match result {
                Ok(g) => {
                    let id = g.guild.id;
                    state.home.set(false);
                    state.guild.set(Some(id));
                    state.channel.set(g.channels.iter().find(|c| c.kind == ChannelKind::Text).map(|c| c.id));
                    state.update_settings(|s| s.last_guild = Some(id));
                    state.modal.set(None);
                    // Keep refreshing even though this dialog is gone now.
                    spawn_forever(async move { state.report(state.refresh().await) });
                }
                Err(e) => super::widgets::set_if_alive(error, Some(e.to_string())),
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
