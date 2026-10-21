//! A guild's channel list: the server menu, categories, text and voice
//! channels with who is in voice, and the dialogs to manage them.

use std::collections::HashSet;

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::api::{GuildWithChannels, UpdateChannelRequest};
use sivryn_client_core::protocol::model::{Channel, ChannelKind, NotifyLevel};
use sivryn_client_core::protocol::permissions::Permissions;
use sivryn_client_core::protocol::{CategoryId, ChannelId, GuildId, UserId};

use super::icons::{Icon, Svg};
use super::widgets::{Avatar, IconButton};
use super::{AppState, MenuItem, Modal, dnd, menu};
use crate::i18n::t;

/// Header with the server name; clicking it unfolds the server menu below.
#[component]
pub fn GuildHeader(guild: GuildId, name: String) -> Element {
    let state = use_context::<AppState>();
    let mut open = use_signal(|| false);
    let perms = state.permissions(guild);
    let owner = state.guild(guild).is_some_and(|g| Some(g.owner_id) == state.me());
    let mut close_and = move |f: Box<dyn FnOnce()>| {
        open.set(false);
        f();
    };

    rsx! {
        div { class: if *open.read() { "sidebar-header clickable open" } else { "sidebar-header clickable" },
            onclick: move |_| open.toggle(),
            span { class: "guild-title", "{name}" }
            if *open.read() { Svg { icon: Icon::X, size: 18 } } else { Svg { icon: Icon::ChevronDown, size: 18 } }
        }
        if *open.read() {
            div { class: "server-menu",
                if perms.allows(Permissions::CREATE_INVITE) {
                    MenuRowButton { label: t("shell.invite"), icon: Icon::UserPlus,
                        onclick: move |_| close_and(Box::new(move || menu::run(state, menu::MenuAction::Invite(guild)))) }
                }
                if perms.allows(Permissions::MANAGE_GUILD) || perms.allows(Permissions::BAN_MEMBERS) || perms.allows(Permissions::MANAGE_ROLES) {
                    MenuRowButton { label: t("guild.settings"), icon: Icon::Settings,
                        onclick: move |_| close_and(Box::new(move || menu::run(state, menu::MenuAction::GuildSettings(guild)))) }
                }
                if perms.allows(Permissions::MANAGE_CHANNELS) {
                    MenuRowButton { label: t("channels.create"), icon: Icon::CirclePlus,
                        onclick: move |_| close_and(Box::new(move || menu::run(state, menu::MenuAction::CreateChannel(guild, None, ChannelKind::Text)))) }
                    MenuRowButton { label: t("channels.create_category"), icon: Icon::Folder,
                        onclick: move |_| close_and(Box::new(move || menu::run(state, menu::MenuAction::CreateCategory(guild)))) }
                }
                div { class: "menu-sep" }
                MenuRowButton { label: t("notify.settings"), icon: Icon::Bell,
                    onclick: move |_| close_and(Box::new(move || menu::run(state, menu::MenuAction::NotifySettings(guild)))) }
                MenuRowButton { label: t("menu.mark_guild_read"), icon: Icon::Check,
                    onclick: move |_| close_and(Box::new(move || menu::run(state, menu::MenuAction::MarkGuildRead(guild)))) }
                if !owner {
                    div { class: "menu-sep" }
                    MenuRowButton { label: t("guild.leave"), icon: Icon::LogOut, danger: true,
                        onclick: move |_| close_and(Box::new(move || menu::run(state, menu::MenuAction::LeaveGuild(guild)))) }
                }
            }
        }
    }
}

#[component]
fn MenuRowButton(
    label: String,
    icon: Icon,
    #[props(default)] danger: bool,
    onclick: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        div { class: if danger { "menu-item danger" } else { "menu-item" }, onclick: move |e| onclick.call(e),
            span { "{label}" }
            Svg { icon, size: 16 }
        }
    }
}

/// Items for a right click on empty space in the channel list.
fn area_items(guild: GuildId) -> Vec<MenuItem> {
    vec![
        MenuItem::action(
            t("channels.create_text"),
            Icon::Hash,
            menu::MenuAction::CreateChannel(guild, None, ChannelKind::Text),
        ),
        MenuItem::action(
            t("channels.create_voice"),
            Icon::Volume,
            menu::MenuAction::CreateChannel(guild, None, ChannelKind::Voice),
        ),
        MenuItem::action(t("channels.create_category"), Icon::Folder, menu::MenuAction::CreateCategory(guild)),
    ]
}

fn category_items(state: AppState, guild: GuildId, category: CategoryId) -> Vec<MenuItem> {
    let collapsed = state.settings.read().collapsed_categories.contains(&category);
    let mut items = vec![MenuItem::action(
        if collapsed { t("channels.expand") } else { t("channels.collapse") },
        if collapsed { Icon::ChevronDown } else { Icon::ChevronRight },
        menu::MenuAction::ToggleCategory(category),
    )];
    if state.permissions(guild).allows(Permissions::MANAGE_CHANNELS) {
        items.push(MenuItem::Separator);
        items.push(MenuItem::action(
            t("channels.create_text"),
            Icon::Hash,
            menu::MenuAction::CreateChannel(guild, Some(category), ChannelKind::Text),
        ));
        items.push(MenuItem::action(
            t("channels.create_voice"),
            Icon::Volume,
            menu::MenuAction::CreateChannel(guild, Some(category), ChannelKind::Voice),
        ));
        items.push(MenuItem::Separator);
        items.push(MenuItem::action(
            t("channels.edit_category"),
            Icon::Pencil,
            menu::MenuAction::EditCategory(category),
        ));
    }
    items
}

#[component]
pub fn ChannelList(guild: GuildWithChannels) -> Element {
    let state = use_context::<AppState>();
    let gid = guild.guild.id;
    let can_manage = state.permissions(gid).allows(Permissions::MANAGE_CHANNELS);
    let collapsed: HashSet<CategoryId> = state.settings.read().collapsed_categories.clone();
    let mut categories = guild.categories.clone();
    categories.sort_by_key(|c| c.position);
    let mut channels = guild.channels.clone();
    // Text channels first within a category, like elsewhere.
    channels.sort_by_key(|c| (c.kind == ChannelKind::Voice, c.position));
    let loose: Vec<Channel> = channels.iter().filter(|c| c.category_id.is_none()).cloned().collect();
    let selected = *state.channel.read();
    let dragging = dnd::active(&state);
    let target = dnd::target(&state);

    rsx! {
        div {
            class: "channel-list",
            onmousedown: move |e| {
                if can_manage && let Some((x, y)) = menu::right_click(&e) {
                    menu::open(state, x, y, area_items(gid));
                }
            },
            for c in loose {
                ChannelRow { key: "{c.id}", channel: c.clone(), selected: selected == Some(c.id), afk: guild.guild.afk_channel_id == Some(c.id) }
            }
            for cat in categories {
                {
                    let id = cat.id;
                    let folded = collapsed.contains(&id);
                    let in_category: Vec<Channel> = channels.iter().filter(|c| c.category_id == Some(id)).cloned().collect();
                    rsx! {
                        div { key: "{id}", class: "category-block",
                            if target == Some(dnd::DropTarget::BeforeCategory(id)) {
                                div { class: "drop-line" }
                            }
                            div {
                                class: "category with-action",
                                onclick: move |_| {
                                    if !dnd::just_dragged() {
                                        menu::run(state, menu::MenuAction::ToggleCategory(id));
                                    }
                                },
                                onmousedown: move |e| {
                                    if let Some((x, y)) = menu::right_click(&e) {
                                        e.stop_propagation();
                                        menu::open(state, x, y, category_items(state, gid, id));
                                    } else if can_manage {
                                        e.stop_propagation();
                                        dnd::begin(state, gid, dnd::DragItem::Category(id), &e);
                                    }
                                },
                                span { class: "category-name",
                                    if folded { Svg { icon: Icon::ChevronRight, size: 12 } } else { Svg { icon: Icon::ChevronDown, size: 12 } }
                                    "{cat.name}"
                                }
                                if can_manage {
                                    div {
                                        class: "category-action",
                                        title: t("channels.create"),
                                        onclick: move |e| {
                                            e.stop_propagation();
                                            menu::run(state, menu::MenuAction::CreateChannel(gid, Some(id), ChannelKind::Text));
                                        },
                                        Svg { icon: Icon::Plus, size: 16 }
                                    }
                                }
                                // Drop zones while dragging: a channel lands at the
                                // top of the category, a category before or after it.
                                match dragging {
                                    Some(dnd::DragItem::Channel(_)) => rsx! {
                                        div { class: "drop-zones",
                                            div { class: "drop-zone full", onmousemove: move |_| dnd::set_target(state, dnd::DropTarget::IntoCategory(id)) }
                                        }
                                    },
                                    Some(dnd::DragItem::Category(_)) => rsx! {
                                        div { class: "drop-zones",
                                            div { class: "drop-zone", onmousemove: move |_| dnd::set_target(state, dnd::DropTarget::BeforeCategory(id)) }
                                            div { class: "drop-zone", onmousemove: move |_| dnd::set_target(state, dnd::DropTarget::AfterCategory(id)) }
                                        }
                                    },
                                    _ => rsx! {},
                                }
                            }
                            if target == Some(dnd::DropTarget::IntoCategory(id)) {
                                div { class: "drop-line" }
                            }
                            for c in in_category {
                                // A folded category still shows the open channel.
                                if !folded || selected == Some(c.id) {
                                    ChannelRow { key: "{c.id}", channel: c.clone(), selected: selected == Some(c.id), afk: guild.guild.afk_channel_id == Some(c.id) }
                                }
                            }
                            if target == Some(dnd::DropTarget::AfterCategory(id)) {
                                div { class: "drop-line" }
                            }
                        }
                    }
                }
            }
        }
    }
}

thread_local! {
    /// When a channel's tool button was last clicked: the row's own click,
    /// which may fire right after, must not undo what the button just did.
    /// A time instead of a flag, so a stopped row click never leaves it set.
    static TOOL_CLICKED: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

fn tool_clicked_just_now() -> bool {
    TOOL_CLICKED.with(|c| c.take()).is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(300))
}

#[component]
fn ChannelRow(channel: Channel, selected: bool, afk: bool) -> Element {
    let mut state = use_context::<AppState>();
    let id = channel.id;
    let gid = channel.guild_id;
    let can_manage = gid.is_some_and(|g| state.permissions(g).allows(Permissions::MANAGE_CHANNELS));
    let unread = state.unread_count(id);
    let muted = state.is_muted(id);
    let voice = channel.kind == ChannelKind::Voice;
    let class = match (selected, unread > 0, muted) {
        (true, _, _) => "channel active",
        (false, _, true) => "channel muted",
        (false, true, _) => "channel unread",
        _ => "channel",
    };
    let dragging = dnd::active(&state);
    let target = dnd::target(&state);
    let can_move_people = gid.is_some_and(|g| state.permissions(g).allows(Permissions::MOVE_MEMBERS));
    let open = move |_| {
        if tool_clicked_just_now() || dnd::just_dragged() {
            return;
        }
        if voice {
            // Voice channels join right away.
            super::call::join_channel(state, id);
            return;
        }
        state.channel.set(Some(id));
        state.voice_chat.set(false);
        state.mark_read(id);
    };

    let drop_class = if voice && target == Some(dnd::DropTarget::Voice(id)) { " drop-into" } else { "" };
    rsx! {
        div {
            if target == Some(dnd::DropTarget::BeforeChannel(id)) {
                div { class: "drop-line" }
            }
            div {
                class: "{class}{drop_class}",
                onclick: open,
                onmousedown: move |e| {
                    if let Some((x, y)) = menu::right_click(&e) {
                        e.stop_propagation();
                        menu::open(state, x, y, menu::channel_items(state, id));
                    } else if let Some(g) = gid.filter(|_| can_manage) {
                        e.stop_propagation();
                        dnd::begin(state, g, dnd::DragItem::Channel(id), &e);
                    }
                },
                if voice { Svg { icon: Icon::Volume, size: 18 } } else { Svg { icon: Icon::Hash, size: 18 } }
                span { class: "channel-name", "{channel.name}" }
                if afk {
                    span { class: "afk-badge", "AFK" }
                }
                if unread > 0 && !selected {
                    span { class: "count-badge", "{unread}" }
                }
                div { class: "channel-tools",
                    if voice {
                        div {
                            class: "channel-tool",
                            title: t("channels.open_chat"),
                            onclick: move |e| {
                                e.stop_propagation();
                                TOOL_CLICKED.with(|c| c.set(Some(std::time::Instant::now())));
                                // The same button closes the chat again.
                                if *state.voice_chat.peek() && *state.channel.peek() == Some(id) {
                                    state.voice_chat.set(false);
                                } else {
                                    menu::run(state, menu::MenuAction::OpenVoiceChat(id));
                                }
                            },
                            Svg { icon: Icon::Message, size: 15 }
                        }
                    }
                    if can_manage {
                        div {
                            class: "channel-tool",
                            title: t("channels.settings"),
                            onclick: move |e| {
                                e.stop_propagation();
                                TOOL_CLICKED.with(|c| c.set(Some(std::time::Instant::now())));
                                state.modal.set(Some(Modal::ChannelSettings(id)));
                            },
                            Svg { icon: Icon::Settings, size: 15 }
                        }
                    }
                }
                match dragging {
                    Some(dnd::DragItem::Channel(_)) => rsx! {
                        div { class: "drop-zones",
                            div { class: "drop-zone", onmousemove: move |_| dnd::set_target(state, dnd::DropTarget::BeforeChannel(id)) }
                            div { class: "drop-zone", onmousemove: move |_| dnd::set_target(state, dnd::DropTarget::AfterChannel(id)) }
                        }
                    },
                    Some(dnd::DragItem::Member { .. }) if voice => rsx! {
                        div { class: "drop-zones",
                            div { class: "drop-zone full", onmousemove: move |_| dnd::set_target(state, dnd::DropTarget::Voice(id)) }
                        }
                    },
                    _ => rsx! {},
                }
            }
            if target == Some(dnd::DropTarget::AfterChannel(id)) {
                div { class: "drop-line" }
            }
            if voice {
                VoiceMembers { channel: id, can_move: can_move_people }
            }
        }
    }
}

/// Who is in a voice channel (the live room while connected to it).
#[component]
fn VoiceMembers(channel: ChannelId, can_move: bool) -> Element {
    let mut state = use_context::<AppState>();
    let me = state.me();
    let dragging = dnd::active(&state);
    // Someone dragged here shows up at the end, half transparent.
    let incoming = match (dragging, dnd::target(&state)) {
        (Some(dnd::DragItem::Member { user, .. }), Some(dnd::DropTarget::Voice(t))) if t == channel => Some(user),
        _ => None,
    };
    let call = state.call.read();
    let voice = state.voice.read();
    let guild = state.guild_of(channel);
    let people: Vec<VoicePerson> = match call.as_ref().filter(|c| c.channel == channel) {
        Some(live) => live
            .participants
            .iter()
            .map(|p| {
                let id: Option<UserId> = p.identity.parse().ok();
                let name = id.map(|id| state.display_name_in(guild, id)).unwrap_or_else(|| p.name.clone());
                VoicePerson {
                    id,
                    name,
                    muted: p.muted,
                    deafened: p.deafened,
                    sharing: p.sharing,
                    speaking: live.speakers.contains(&p.identity),
                }
            })
            .collect(),
        None => voice
            .get(&channel)
            .into_iter()
            .flatten()
            .map(|p| VoicePerson {
                id: Some(p.user_id),
                name: state.display_name_in(guild, p.user_id),
                muted: p.muted,
                deafened: p.deafened,
                sharing: p.screen_sharing,
                speaking: false,
            })
            .collect(),
    };
    drop(call);
    drop(voice);
    let incoming = incoming.filter(|u| !people.iter().any(|p| p.id == Some(*u)));
    if people.is_empty() && incoming.is_none() {
        return rsx! {};
    }
    rsx! {
        div { class: "voice-members",
            for (i, p) in people.into_iter().enumerate() {
                {
                    let VoicePerson { id, name, muted, deafened, sharing, speaking } = p;
                    let draggable = id.is_some_and(|u| can_move || Some(u) == me);
                    rsx! {
                        div {
                            key: "{i}",
                            class: "voice-member",
                            // Left click: the profile.
                            onclick: move |_| {
                                if let Some(user) = id.filter(|_| !dnd::just_dragged()) {
                                    state.modal.set(Some(Modal::Profile(user)));
                                }
                            },
                            onmousedown: move |e| {
                                let Some(user) = id else { return };
                                if let Some((x, y)) = menu::right_click(&e) {
                                    e.stop_propagation();
                                    menu::open(state, x, y, menu::user_items(state, user, guild, Some(channel)));
                                } else if let Some(g) = guild.filter(|_| draggable) {
                                    e.stop_propagation();
                                    dnd::begin(state, g, dnd::DragItem::Member { user, from: channel }, &e);
                                }
                            },
                            Avatar { name: name.clone(), id, size: 22, speaking }
                            span { class: "voice-member-name", "{name}" }
                            if sharing {
                                span { class: "live-badge", "LIVE" }
                            }
                            if deafened {
                                span { class: "member-muted", Svg { icon: Icon::HeadphonesOff, size: 14 } }
                            } else if muted {
                                span { class: "member-muted", Svg { icon: Icon::MicOff, size: 14 } }
                            }
                            if matches!(dragging, Some(dnd::DragItem::Member { .. })) {
                                div { class: "drop-zones",
                                    div { class: "drop-zone full", onmousemove: move |_| dnd::set_target(state, dnd::DropTarget::Voice(channel)) }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(user) = incoming {
                div { class: "voice-member incoming",
                    Avatar { name: state.display_name_in(guild, user), id: Some(user), size: 22 }
                    span { class: "voice-member-name", "{state.display_name_in(guild, user)}" }
                }
            }
        }
    }
}

/// One row in a voice channel's member list.
#[derive(Clone, PartialEq)]
struct VoicePerson {
    id: Option<UserId>,
    name: String,
    muted: bool,
    deafened: bool,
    sharing: bool,
    speaking: bool,
}

/// Text or voice channel, optionally in a category.
#[component]
pub fn CreateChannelDialog(guild: GuildId, category: Option<CategoryId>, kind: ChannelKind) -> Element {
    let mut state = use_context::<AppState>();
    let mut name = use_signal(String::new);
    let mut kind = use_signal(move || kind);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let category_name = category.and_then(|c| {
        state.guilds.read().iter().flat_map(|g| &g.categories).find(|x| x.id == c).map(|x| x.name.clone())
    });
    let mut create = move || {
        let Some(s) = state.session.cloned() else { return };
        let value = name.read().trim().to_owned();
        if value.is_empty() || *busy.peek() {
            return;
        }
        busy.set(true);
        let req = sivryn_client_core::protocol::api::CreateChannelRequest {
            name: value,
            kind: *kind.peek(),
            e2ee: false,
            category_id: category,
        };
        spawn(async move {
            let result = s.api.create_channel(guild, &req).await;
            busy.set(false);
            match result {
                Ok(channel) => {
                    state.modal.set(None);
                    spawn_forever(async move {
                        state.report(state.refresh().await);
                        state.channel.set(Some(channel.id));
                    });
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    let current = *kind.read();

    rsx! {
        div { class: "overlay", onclick: move |_| state.modal.set(None),
            div { class: "dialog", onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { {t("channels.create")} }
                    IconButton { icon: Icon::X, size: 18, onclick: move |_| state.modal.set(None) }
                }
                if let Some(cat) = category_name {
                    p { class: "muted", "{t(\"channels.in_category\")} {cat}" }
                }
                div { class: "field-label", {t("channels.type")} }
                div { class: "kind-options",
                    div { class: if current == ChannelKind::Text { "kind-option active" } else { "kind-option" },
                        onclick: move |_| kind.set(ChannelKind::Text),
                        Svg { icon: Icon::Hash, size: 22 }
                        div {
                            div { class: "kind-title", {t("channels.text")} }
                            div { class: "muted small", {t("channels.text_hint")} }
                        }
                    }
                    div { class: if current == ChannelKind::Voice { "kind-option active" } else { "kind-option" },
                        onclick: move |_| kind.set(ChannelKind::Voice),
                        Svg { icon: Icon::Volume, size: 22 }
                        div {
                            div { class: "kind-title", {t("channels.voice")} }
                            div { class: "muted small", {t("channels.voice_hint")} }
                        }
                    }
                }
                div { class: "field-label", {t("channels.name")} }
                input {
                    class: "input",
                    autofocus: "true",
                    value: "{name}",
                    placeholder: if current == ChannelKind::Text { "neuer-kanal" } else { "Sprache" },
                    oninput: move |e| name.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create();
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
                    button { class: if *busy.read() { "button primary busy" } else { "button primary" }, onclick: move |_| create(),
                        {t("channels.create_action")}
                    }
                }
            }
        }
    }
}

/// Creates a category, or renames / deletes one (`existing`).
#[component]
pub fn CategoryDialog(guild: GuildId, existing: Option<CategoryId>) -> Element {
    let mut state = use_context::<AppState>();
    let current_name = existing
        .and_then(|c| {
            state.guilds.peek().iter().flat_map(|g| &g.categories).find(|x| x.id == c).map(|x| x.name.clone())
        })
        .unwrap_or_default();
    let mut name = use_signal(move || current_name);
    let mut busy = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut save = move || {
        let Some(s) = state.session.cloned() else { return };
        let value = name.read().trim().to_owned();
        if value.is_empty() || *busy.peek() {
            return;
        }
        busy.set(true);
        spawn(async move {
            let result = match existing {
                Some(c) => s.api.rename_category(c, &value).await,
                None => s.api.create_category(guild, &value).await.map(|_| ()),
            };
            busy.set(false);
            match result {
                Ok(()) => {
                    state.modal.set(None);
                    spawn_forever(async move { state.report(state.refresh().await) });
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    let delete = move |_| {
        let (Some(s), Some(c)) = (state.session.cloned(), existing) else { return };
        if !*confirm_delete.peek() {
            confirm_delete.set(true);
            return;
        }
        spawn_forever(async move {
            state.report(s.api.delete_category(c).await.map_err(|e| e.to_string()));
            state.modal.set(None);
            state.report(state.refresh().await);
        });
    };

    rsx! {
        div { class: "overlay", onclick: move |_| state.modal.set(None),
            div { class: "dialog", onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { if existing.is_some() { {t("channels.edit_category")} } else { {t("channels.create_category")} } }
                    IconButton { icon: Icon::X, size: 18, onclick: move |_| state.modal.set(None) }
                }
                div { class: "field-label", {t("channels.category_name")} }
                input {
                    class: "input",
                    autofocus: "true",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            save();
                        } else if e.key() == Key::Escape {
                            state.modal.set(None);
                        }
                    },
                }
                if *confirm_delete.read() {
                    div { class: "form-error", {t("channels.delete_category_hint")} }
                }
                if let Some(err) = error.read().as_ref() {
                    div { class: "form-error", "{err}" }
                }
                div { class: "dialog-actions",
                    if existing.is_some() {
                        button { class: "button danger-ghost", onclick: delete,
                            if *confirm_delete.read() { {t("channels.delete_confirm")} } else { {t("channels.delete_category")} }
                        }
                        span { class: "topbar-spacer" }
                    }
                    button { class: "button ghost", onclick: move |_| state.modal.set(None), {t("common.cancel")} }
                    button { class: if *busy.read() { "button primary busy" } else { "button primary" }, onclick: move |_| save(),
                        if existing.is_some() { {t("common.save")} } else { {t("channels.create_action")} }
                    }
                }
            }
        }
    }
}

/// Rename or delete a channel.
#[component]
pub fn ChannelSettingsDialog(channel: ChannelId) -> Element {
    let mut state = use_context::<AppState>();
    let info = state.find_channel(channel);
    let mut name = use_signal(move || info.as_ref().map(|c| c.name.clone()).unwrap_or_default());
    let mut busy = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut save = move || {
        let Some(s) = state.session.cloned() else { return };
        let value = name.read().trim().to_owned();
        if value.is_empty() || *busy.peek() {
            return;
        }
        busy.set(true);
        spawn(async move {
            let req = UpdateChannelRequest { name: Some(value), ..Default::default() };
            let result = s.api.update_channel(channel, &req).await;
            busy.set(false);
            match result {
                Ok(_) => {
                    state.modal.set(None);
                    spawn_forever(async move { state.report(state.refresh().await) });
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    let delete = move |_| {
        let Some(s) = state.session.cloned() else { return };
        if !*confirm_delete.peek() {
            confirm_delete.set(true);
            return;
        }
        spawn_forever(async move {
            state.report(s.api.delete_channel(channel).await.map_err(|e| e.to_string()));
            state.modal.set(None);
            if *state.channel.peek() == Some(channel) {
                state.channel.set(None);
            }
            state.report(state.refresh().await);
        });
    };

    rsx! {
        div { class: "overlay", onclick: move |_| state.modal.set(None),
            div { class: "dialog", onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { {t("channels.settings")} }
                    IconButton { icon: Icon::X, size: 18, onclick: move |_| state.modal.set(None) }
                }
                div { class: "field-label", {t("channels.name")} }
                input {
                    class: "input",
                    autofocus: "true",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            save();
                        } else if e.key() == Key::Escape {
                            state.modal.set(None);
                        }
                    },
                }
                if *confirm_delete.read() {
                    div { class: "form-error", {t("channels.delete_hint")} }
                }
                if let Some(err) = error.read().as_ref() {
                    div { class: "form-error", "{err}" }
                }
                div { class: "dialog-actions",
                    button { class: "button danger-ghost", onclick: delete,
                        if *confirm_delete.read() { {t("channels.delete_confirm")} } else { {t("channels.delete")} }
                    }
                    span { class: "topbar-spacer" }
                    button { class: "button ghost", onclick: move |_| state.modal.set(None), {t("common.cancel")} }
                    button { class: if *busy.read() { "button primary busy" } else { "button primary" }, onclick: move |_| save(), {t("common.save")} }
                }
            }
        }
    }
}

/// Which messages in a guild notify you.
#[component]
pub fn NotifyDialog(guild: GuildId) -> Element {
    let mut state = use_context::<AppState>();
    let current = state.guilds.read().iter().find(|g| g.guild.id == guild).map(|g| g.notify_level).unwrap_or_default();
    let name = state.guild(guild).map(|g| g.name).unwrap_or_default();
    let set = move |level: NotifyLevel| {
        let mut state = state;
        let Some(s) = state.session.cloned() else { return };
        // Show the choice right away; the server confirms in the background.
        if let Some(g) = state.guilds.write().iter_mut().find(|g| g.guild.id == guild) {
            g.notify_level = level;
        }
        spawn_forever(async move {
            state.report(s.api.update_my_member(guild, None, Some(level)).await.map_err(|e| e.to_string()));
        });
    };
    let option = |level: NotifyLevel, label: &'static str, hint: &'static str| {
        let active = current == level;
        rsx! {
            div { class: if active { "radio-row active" } else { "radio-row" }, onclick: move |_| set(level),
                div { class: if active { "radio on" } else { "radio" } }
                div {
                    div { class: "kind-title", "{label}" }
                    div { class: "muted small", "{hint}" }
                }
            }
        }
    };

    rsx! {
        div { class: "overlay", onclick: move |_| state.modal.set(None),
            div { class: "dialog", onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { {t("notify.settings")} }
                    IconButton { icon: Icon::X, size: 18, onclick: move |_| state.modal.set(None) }
                }
                p { class: "muted", "{name}" }
                {option(NotifyLevel::All, t("notify.level_all"), t("notify.level_all_hint"))}
                {option(NotifyLevel::Mentions, t("notify.level_mentions"), t("notify.level_mentions_hint"))}
                {option(NotifyLevel::Nothing, t("notify.level_nothing"), t("notify.level_nothing_hint"))}
                div { class: "dialog-actions",
                    button { class: "button primary", onclick: move |_| state.modal.set(None), {t("common.done")} }
                }
            }
        }
    }
}
