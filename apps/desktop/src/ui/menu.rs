//! Right-click menus. Items carry a [`MenuAction`] (plain data) that
//! [`run`] carries out, so a menu keeps working even after the component that
//! opened it is gone.

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::permissions::Permissions;
use sivryn_client_core::protocol::{ChannelId, GuildId, MessageId, UserId};

use super::icons::{Icon, Svg};
use super::widgets::Slider;
use super::{AppState, Modal};
use crate::i18n::t;
use crate::messages::ChatMessage;

#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Reply(ChannelId, MessageId),
    Edit(ChannelId, MessageId),
    Delete(ChannelId, MessageId),
    React(ChannelId, MessageId),
    QuickReact(ChannelId, MessageId, String),
    Copy(String),
    OpenLink(String),
    Profile(UserId),
    Message(UserId),
    AddFriend(String),
    AcceptFriend(UserId),
    RemoveFriend(UserId),
    Block(UserId),
    Unblock(UserId),
    Kick(GuildId, UserId),
    Ban(GuildId, UserId),
    Disconnect(ChannelId, UserId),
    ToggleLocalMute(String),
    GuildSettings(GuildId),
    Invite(GuildId),
    LeaveGuild(GuildId),
    MarkRead(ChannelId),
    AddToDm(ChannelId),
    LeaveDm(ChannelId),
    Call(ChannelId),
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuItem {
    Action {
        label: String,
        icon: Option<Icon>,
        danger: bool,
        action: MenuAction,
    },
    Check {
        label: String,
        checked: bool,
        action: MenuAction,
    },
    /// A 0–200 % volume slider bound to a key in the settings.
    Volume {
        label: String,
        key: String,
    },
    /// A row of quick reactions.
    Reactions {
        channel: ChannelId,
        message: MessageId,
    },
    Separator,
}

impl MenuItem {
    fn action(label: &str, icon: Icon, action: MenuAction) -> Self {
        MenuItem::Action { label: label.into(), icon: Some(icon), danger: false, action }
    }

    fn danger(label: &str, icon: Icon, action: MenuAction) -> Self {
        MenuItem::Action { label: label.into(), icon: Some(icon), danger: true, action }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenu {
    pub x: f64,
    pub y: f64,
    pub items: Vec<MenuItem>,
}

/// Where a right click happened, or `None` for other buttons.
pub fn right_click(e: &MouseEvent) -> Option<(f64, f64)> {
    use dioxus_native::prelude::dioxus_elements::input_data::MouseButton;
    if e.trigger_button() != Some(MouseButton::Secondary) {
        return None;
    }
    let p = e.client_coordinates();
    Some((p.x, p.y))
}

pub fn open(mut state: AppState, x: f64, y: f64, items: Vec<MenuItem>) {
    // Drop separators at the ends and doubled ones left by missing items.
    let mut cleaned: Vec<MenuItem> = Vec::new();
    for item in items {
        if item == MenuItem::Separator && cleaned.last().is_none_or(|l| *l == MenuItem::Separator) {
            continue;
        }
        cleaned.push(item);
    }
    if cleaned.last() == Some(&MenuItem::Separator) {
        cleaned.pop();
    }
    if !cleaned.is_empty() {
        state.menu.set(Some(ContextMenu { x, y, items: cleaned }));
    }
}

/// Menu for a message.
pub fn message_items(
    state: AppState,
    channel: ChannelId,
    msg: &ChatMessage,
    selected_link: Option<String>,
) -> Vec<MenuItem> {
    let me = state.me();
    let mine = Some(msg.author) == me;
    let can_moderate =
        state.guild_of(channel).is_some_and(|g| state.permissions(g).allows(Permissions::MANAGE_MESSAGES));
    let mut items = vec![MenuItem::Reactions { channel, message: msg.id }];
    items.push(MenuItem::action(t("menu.add_reaction"), Icon::Smile, MenuAction::React(channel, msg.id)));
    if !mine {
        items.push(MenuItem::action(t("menu.reply"), Icon::Reply, MenuAction::Reply(channel, msg.id)));
    }
    if mine && msg.content.is_some() {
        items.push(MenuItem::action(t("menu.edit"), Icon::Pencil, MenuAction::Edit(channel, msg.id)));
    }
    if let Some(selected) = blitz_dom::text_selection::selected() {
        items.push(MenuItem::action(t("menu.copy_selection"), Icon::Copy, MenuAction::Copy(selected)));
    }
    if let Some(body) = msg.body().filter(|b| !b.is_empty()) {
        items.push(MenuItem::action(t("menu.copy_text"), Icon::Copy, MenuAction::Copy(body.to_owned())));
    }
    if let Some(link) = selected_link {
        items.push(MenuItem::action(t("menu.open_link"), Icon::External, MenuAction::OpenLink(link.clone())));
        items.push(MenuItem::action(t("menu.copy_link"), Icon::Link, MenuAction::Copy(link)));
    }
    items.push(MenuItem::Separator);
    items.push(MenuItem::action(t("menu.profile"), Icon::User, MenuAction::Profile(msg.author)));
    if mine || can_moderate {
        items.push(MenuItem::Separator);
        items.push(MenuItem::danger(t("menu.delete"), Icon::Trash, MenuAction::Delete(channel, msg.id)));
    }
    items
}

/// Menu for a user. `guild` adds moderation; `voice` adds call controls.
pub fn user_items(state: AppState, user: UserId, guild: Option<GuildId>, voice: Option<ChannelId>) -> Vec<MenuItem> {
    let Some(me) = state.me() else { return Vec::new() };
    if user == me {
        return vec![MenuItem::action(t("menu.profile"), Icon::User, MenuAction::Profile(user))];
    }
    let name = state.username(user);
    let friends = state.friends.read();
    let is_friend = friends.friends.iter().any(|f| f.id == user);
    let blocked = friends.blocked.iter().any(|f| f.id == user);
    let pending = friends.outgoing.iter().any(|f| f.id == user);
    let incoming = friends.incoming.iter().any(|f| f.id == user);
    drop(friends);

    let mut items = vec![
        MenuItem::action(t("menu.profile"), Icon::User, MenuAction::Profile(user)),
        MenuItem::action(t("menu.message"), Icon::Message, MenuAction::Message(user)),
    ];
    if is_friend {
        items.push(MenuItem::action(t("menu.remove_friend"), Icon::UserMinus, MenuAction::RemoveFriend(user)));
    } else if incoming {
        items.push(MenuItem::action(t("friends.accept"), Icon::Check, MenuAction::AcceptFriend(user)));
    } else if !blocked && !pending {
        items.push(MenuItem::action(t("menu.add_friend"), Icon::UserPlus, MenuAction::AddFriend(name)));
    }
    if voice.is_some() {
        let key = user.to_string();
        let muted = state.settings.read().muted_users.contains(&key);
        items.push(MenuItem::Separator);
        items.push(MenuItem::Volume { label: t("call.user_volume").into(), key: key.clone() });
        items.push(MenuItem::Check {
            label: t("menu.mute_user").into(),
            checked: muted,
            action: MenuAction::ToggleLocalMute(key),
        });
    }
    if let Some(guild) = guild {
        let perms = state.permissions(guild);
        let owner = state.guild(guild).map(|g| g.owner_id);
        let target_is_owner = owner == Some(user);
        let mut moderation = Vec::new();
        if let Some(channel) = voice.filter(|_| perms.allows(Permissions::MOVE_MEMBERS)) {
            moderation.push(MenuItem::danger(
                t("menu.disconnect"),
                Icon::PhoneOff,
                MenuAction::Disconnect(channel, user),
            ));
        }
        if !target_is_owner && perms.allows(Permissions::KICK_MEMBERS) {
            moderation.push(MenuItem::danger(t("menu.kick"), Icon::UserMinus, MenuAction::Kick(guild, user)));
        }
        if !target_is_owner && perms.allows(Permissions::BAN_MEMBERS) {
            moderation.push(MenuItem::danger(t("menu.ban"), Icon::Ban, MenuAction::Ban(guild, user)));
        }
        if !moderation.is_empty() {
            items.push(MenuItem::Separator);
            items.extend(moderation);
        }
    }
    items.push(MenuItem::Separator);
    if blocked {
        items.push(MenuItem::action(t("menu.unblock"), Icon::Ban, MenuAction::Unblock(user)));
    } else {
        items.push(MenuItem::danger(t("menu.block"), Icon::Ban, MenuAction::Block(user)));
    }
    items
}

pub fn guild_items(state: AppState, guild: GuildId) -> Vec<MenuItem> {
    let perms = state.permissions(guild);
    let mine = state.guild(guild).is_some_and(|g| Some(g.owner_id) == state.me());
    let mut items = Vec::new();
    if perms.allows(Permissions::CREATE_INVITE) {
        items.push(MenuItem::action(t("shell.invite"), Icon::UserPlus, MenuAction::Invite(guild)));
    }
    if perms.allows(Permissions::MANAGE_GUILD) || perms.allows(Permissions::BAN_MEMBERS) {
        items.push(MenuItem::action(t("guild.settings"), Icon::Settings, MenuAction::GuildSettings(guild)));
    }
    if !mine {
        items.push(MenuItem::Separator);
        items.push(MenuItem::danger(t("guild.leave"), Icon::LogOut, MenuAction::LeaveGuild(guild)));
    }
    items
}

pub fn channel_items(state: AppState, channel: ChannelId) -> Vec<MenuItem> {
    let mut items = Vec::new();
    if state.unread.read().contains(&channel) {
        items.push(MenuItem::action(t("menu.mark_read"), Icon::Check, MenuAction::MarkRead(channel)));
    }
    if let Some(dm) = state.dm(channel) {
        items.push(MenuItem::action(t("call.start"), Icon::Phone, MenuAction::Call(channel)));
        items.push(MenuItem::action(t("dm.add_people"), Icon::UserPlus, MenuAction::AddToDm(channel)));
        if dm.channel.kind == sivryn_client_core::protocol::model::ChannelKind::Group {
            items.push(MenuItem::Separator);
            items.push(MenuItem::danger(t("dm.leave"), Icon::LogOut, MenuAction::LeaveDm(channel)));
        }
    }
    items
}

pub fn copy_to_clipboard(text: &str) {
    match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_owned())) {
        Ok(()) => {}
        Err(e) => tracing::warn!(error = %e, "clipboard unavailable"),
    }
}

/// Carries out a menu action.
/// Runs a task that outlives the menu.
fn bg(f: impl std::future::Future<Output = ()> + 'static) {
    spawn_forever(f);
}

pub fn run(mut state: AppState, action: MenuAction) {
    state.menu.set(None);
    let Some(s) = state.session.cloned() else { return };
    let done = move |result: Result<(), String>| state.report(result);
    match action {
        MenuAction::Reply(channel, message) => {
            let mut c = state.composer.write();
            let composer = c.entry(channel).or_default();
            composer.editing = None;
            composer.reply_to = Some(message);
        }
        MenuAction::Edit(channel, message) => super::chat_view::start_edit(state, channel, message),
        MenuAction::Delete(channel, message) => state.modal.set(Some(Modal::ConfirmDelete(channel, message))),
        MenuAction::React(channel, message) => super::emoji_picker::open_for_reaction(state, channel, message),
        MenuAction::QuickReact(channel, message, emoji) => {
            super::chat_view::toggle_reaction(state, channel, message, emoji)
        }
        MenuAction::Copy(text) => {
            copy_to_clipboard(&text);
            state.toast(t("toast.copied"), false);
        }
        MenuAction::OpenLink(url) => crate::files::open_link(&url),
        MenuAction::Profile(user) => state.modal.set(Some(Modal::Profile(user))),
        MenuAction::Message(user) => state.message_users(vec![user]),
        MenuAction::AddFriend(name) => bg(async move {
            match s.api.send_friend_request(&name).await {
                Ok(()) => state.toast(t("friends.request_sent"), false),
                Err(e) => done(Err(e.to_string())),
            }
        }),
        MenuAction::AcceptFriend(user) => {
            bg(async move { done(s.api.accept_friend(user).await.map_err(|e| e.to_string())) })
        }
        MenuAction::RemoveFriend(user) => {
            bg(async move { done(s.api.remove_friend(user).await.map_err(|e| e.to_string())) })
        }
        MenuAction::Block(user) => bg(async move { done(s.api.block(user).await.map_err(|e| e.to_string())) }),
        MenuAction::Unblock(user) => bg(async move { done(s.api.unblock(user).await.map_err(|e| e.to_string())) }),
        MenuAction::Kick(guild, user) => state.modal.set(Some(Modal::ConfirmModeration { guild, user, ban: false })),
        MenuAction::Ban(guild, user) => state.modal.set(Some(Modal::ConfirmModeration { guild, user, ban: true })),
        MenuAction::Disconnect(channel, user) => {
            bg(async move { done(s.api.disconnect_voice(channel, user).await.map_err(|e| e.to_string())) })
        }
        MenuAction::ToggleLocalMute(key) => state.update_settings(|st| {
            if !st.muted_users.remove(&key) {
                st.muted_users.insert(key);
            }
        }),
        MenuAction::GuildSettings(guild) => state.modal.set(Some(Modal::GuildSettings(guild))),
        MenuAction::Invite(guild) => {
            state.guild.set(Some(guild));
            state.modal.set(Some(Modal::Invite));
        }
        MenuAction::LeaveGuild(guild) => {
            bg(async move {
                done(s.api.remove_member(guild, s.user.id).await.map_err(|e| e.to_string()));
                done(state.refresh().await);
            });
        }
        MenuAction::MarkRead(channel) => {
            state.unread.write().remove(&channel);
        }
        MenuAction::AddToDm(channel) => state.modal.set(Some(Modal::AddToDm(channel))),
        MenuAction::LeaveDm(channel) => bg(async move {
            done(s.api.leave_dm(channel).await.map_err(|e| e.to_string()));
            if *state.channel.peek() == Some(channel) {
                state.open_home();
            }
            done(state.refresh_dms().await);
        }),
        MenuAction::Call(channel) => {
            state.open_channel(channel);
            super::call::start(state, channel, false);
        }
    }
}

#[component]
pub fn ContextMenuView() -> Element {
    let mut state = use_context::<AppState>();
    let Some(menu) = state.menu.read().clone() else { return rsx! {} };
    // Keep the menu on screen: open leftwards/upwards near the edges.
    let height: f64 = menu
        .items
        .iter()
        .map(|i| match i {
            MenuItem::Separator => 9.0,
            MenuItem::Volume { .. } => 58.0,
            MenuItem::Reactions { .. } => 45.0,
            _ => 32.0,
        })
        .sum::<f64>()
        + 14.0;
    let (mut x, mut y) = (menu.x, menu.y);
    if let Some((w, h)) = crate::window::client_size() {
        if x + 226.0 > w {
            x = (x - 222.0).max(4.0);
        }
        if y + height > h {
            y = (h - height - 6.0).max(4.0);
        }
    }
    let style = format!("left: {}px; top: {}px;", x.max(4.0), y.max(4.0));

    rsx! {
        div { class: "menu-backdrop",
            onmousedown: move |_| state.menu.set(None),
        }
        div { class: "context-menu", style: "{style}",
            onmousedown: move |e| e.stop_propagation(),
            for (i, item) in menu.items.into_iter().enumerate() {
                MenuRow { key: "{i}", item }
            }
        }
    }
}

#[component]
fn MenuRow(item: MenuItem) -> Element {
    let state = use_context::<AppState>();
    match item {
        MenuItem::Separator => rsx! { div { class: "menu-sep" } },
        MenuItem::Action { label, icon, danger, action } => rsx! {
            div {
                class: if danger { "menu-item danger" } else { "menu-item" },
                onclick: move |_| run(state, action.clone()),
                span { "{label}" }
                if let Some(icon) = icon {
                    Svg { icon, size: 16 }
                }
            }
        },
        MenuItem::Check { label, checked, action } => rsx! {
            div { class: "menu-item", onclick: move |_| run(state, action.clone()),
                span { "{label}" }
                div { class: if checked { "menu-check on" } else { "menu-check" },
                    if checked {
                        Svg { icon: Icon::Check, size: 14 }
                    }
                }
            }
        },
        MenuItem::Volume { label, key } => {
            let value = state.settings.read().volumes.get(&key).copied().unwrap_or(1.0);
            rsx! {
                div { class: "menu-volume",
                    div { class: "menu-row",
                        span { "{label}" }
                        span { class: "muted small", "{(value * 100.0).round()} %" }
                    }
                    Slider {
                        value,
                        min: 0.0,
                        max: 2.0,
                        steps: 40,
                        onchange: move |v: f32| {
                            let key = key.clone();
                            state.update_settings(|s| {
                                s.volumes.insert(key, v);
                            });
                        },
                    }
                }
            }
        }
        MenuItem::Reactions { channel, message } => rsx! {
            div { class: "menu-reactions",
                for emoji in crate::emoji::QUICK.iter().take(6) {
                    div {
                        key: "{emoji}",
                        class: "menu-reaction emoji",
                        onclick: move |_| run(state, MenuAction::QuickReact(channel, message, emoji.to_string())),
                        "{emoji}"
                    }
                }
            }
        },
    }
}
