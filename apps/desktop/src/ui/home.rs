//! The home view: friends and direct messages.

use std::collections::HashSet;

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::model::{ChannelKind, User};
use sivryn_client_core::protocol::{ChannelId, UserId};

use super::icons::{Icon, Svg};
use super::widgets::{Avatar, IconButton};
use super::{AppState, Modal, menu};
use crate::i18n::t;

fn display_name(u: &User) -> String {
    u.display_name.clone().unwrap_or_else(|| u.username.clone())
}

/// Sidebar content in the home view.
#[component]
pub fn HomeSidebar() -> Element {
    let mut state = use_context::<AppState>();
    let selected = *state.channel.read();
    let incoming = state.friends.read().incoming.len();
    let unread = state.unread.read().clone();
    let me = state.me();
    let dms = state.dms.read().clone();

    rsx! {
        div { class: "channel-list",
            div {
                class: if selected.is_none() { "home-link active" } else { "home-link" },
                onclick: move |_| state.open_home(),
                Svg { icon: Icon::Users, size: 20 }
                span { class: "channel-name", {t("friends.title")} }
                if incoming > 0 {
                    span { class: "count-badge", "{incoming}" }
                }
            }
            div { class: "category with-action",
                span { {t("home.direct_messages")} }
                div { class: "category-action", title: t("home.new_dm"), onclick: move |_| state.modal.set(Some(Modal::NewDm)),
                    Svg { icon: Icon::Plus, size: 16 }
                }
            }
            if dms.is_empty() {
                div { class: "sidebar-empty muted small", {t("home.no_dms")} }
            }
            for dm in dms {
                {
                    let id = dm.channel.id;
                    let title = state.dm_title(&dm);
                    let group = dm.channel.kind == ChannelKind::Group;
                    let partner = dm.members.iter().find(|u| Some(u.id) != me).map(|u| u.id);
                    let online = partner.is_some_and(|p| state.online.read().contains(&p));
                    let count = dm.members.len();
                    let is_unread = unread.contains(&id) && selected != Some(id);
                    rsx! {
                        div {
                            key: "{id}",
                            class: if selected == Some(id) { "dm-entry active" } else if is_unread { "dm-entry unread" } else { "dm-entry" },
                            onclick: move |_| state.open_channel(id),
                            onmousedown: move |e| {
                                if let Some((x, y)) = menu::right_click(&e) {
                                    let mut items = menu::channel_items(state, id);
                                    if let Some(p) = partner.filter(|_| !group) {
                                        items.push(super::MenuItem::Separator);
                                        items.extend(menu::user_items(state, p, None, None));
                                    }
                                    menu::open(state, x, y, items);
                                }
                            },
                            div { class: "member-avatar",
                                if group {
                                    div { class: "group-avatar", Svg { icon: Icon::Users, size: 18 } }
                                } else {
                                    Avatar { name: title.clone(), id: partner, size: 32 }
                                    div { class: if online { "presence-dot online" } else { "presence-dot" } }
                                }
                            }
                            div { class: "dm-text",
                                div { class: "dm-title", "{title}" }
                                if group {
                                    div { class: "muted small", "{count} {t(\"home.members\")}" }
                                }
                            }
                            if is_unread {
                                div { class: "unread-dot" }
                            }
                            div {
                                class: "dm-close",
                                title: t("home.close_dm"),
                                onclick: move |e| {
                                    e.stop_propagation();
                                    state.dms.write().retain(|d| d.channel.id != id);
                                    if *state.channel.peek() == Some(id) {
                                        state.open_home();
                                    }
                                },
                                Svg { icon: Icon::X, size: 14 }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Online,
    All,
    Pending,
    Blocked,
    Add,
}

#[component]
pub fn FriendsPage() -> Element {
    let state = use_context::<AppState>();
    let mut tab = use_signal(|| Tab::Online);
    let friends = state.friends.read().clone();
    let online = state.online.read().clone();
    let pending = friends.incoming.len() + friends.outgoing.len();
    let current = *tab.read();
    let tab_class = move |t: Tab| if current == t { "tab active" } else { "tab" };

    rsx! {
        div { class: "friends",
            header { class: "topbar",
                span { class: "topbar-icon", Svg { icon: Icon::Users, size: 20 } }
                span { class: "topbar-title", {t("friends.title")} }
                div { class: "topbar-sep" }
                div { class: tab_class(Tab::Online), onclick: move |_| tab.set(Tab::Online), {t("friends.online")} }
                div { class: tab_class(Tab::All), onclick: move |_| tab.set(Tab::All), {t("friends.all")} }
                div { class: tab_class(Tab::Pending), onclick: move |_| tab.set(Tab::Pending),
                    {t("friends.pending")}
                    if !friends.incoming.is_empty() {
                        span { class: "count-badge", "{friends.incoming.len()}" }
                    }
                }
                div { class: tab_class(Tab::Blocked), onclick: move |_| tab.set(Tab::Blocked), {t("friends.blocked")} }
                div { class: if current == Tab::Add { "tab add active" } else { "tab add" }, onclick: move |_| tab.set(Tab::Add), {t("friends.add")} }
            }
            div { class: "friends-body",
                match current {
                    Tab::Add => rsx! { AddFriend {} },
                    Tab::Online => {
                        let list: Vec<User> = friends.friends.iter().filter(|u| online.contains(&u.id)).cloned().collect();
                        rsx! { FriendList { title: t("friends.online").to_string(), users: list, kind: RowKind::Friend, empty: t("friends.none_online").to_string() } }
                    }
                    Tab::All => rsx! { FriendList { title: t("friends.all").to_string(), users: friends.friends.clone(), kind: RowKind::Friend, empty: t("friends.none").to_string() } },
                    Tab::Pending => rsx! {
                        if pending == 0 {
                            div { class: "friends-empty muted", {t("friends.none_pending")} }
                        }
                        if !friends.incoming.is_empty() {
                            FriendList { title: t("friends.incoming").to_string(), users: friends.incoming.clone(), kind: RowKind::Incoming, empty: String::new() }
                        }
                        if !friends.outgoing.is_empty() {
                            FriendList { title: t("friends.outgoing").to_string(), users: friends.outgoing.clone(), kind: RowKind::Outgoing, empty: String::new() }
                        }
                    },
                    Tab::Blocked => rsx! { FriendList { title: t("friends.blocked").to_string(), users: friends.blocked.clone(), kind: RowKind::Blocked, empty: t("privacy.blocked_none").to_string() } },
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Friend,
    Incoming,
    Outgoing,
    Blocked,
}

#[component]
fn FriendList(title: String, users: Vec<User>, kind: RowKind, empty: String) -> Element {
    let mut users = users;
    users.sort_by_key(|u| display_name(u).to_lowercase());
    if users.is_empty() {
        return rsx! { div { class: "friends-empty muted", "{empty}" } };
    }
    rsx! {
        div { class: "friends-heading", "{title} — {users.len()}" }
        for u in users {
            FriendRow { key: "{u.id}", user: u.clone(), kind }
        }
    }
}

fn act(
    state: AppState,
    f: impl AsyncFnOnce(&crate::session::Session) -> Result<(), sivryn_client_core::ClientError> + 'static,
) {
    let Some(s) = state.session.cloned() else { return };
    spawn_forever(async move {
        state.report(f(&s).await.map_err(|e| e.to_string()));
        state.report(state.refresh_friends().await);
    });
}

#[component]
fn FriendRow(user: User, kind: RowKind) -> Element {
    let state = use_context::<AppState>();
    let name = display_name(&user);
    let id = user.id;
    let online = state.online.read().contains(&id);
    let status = match kind {
        RowKind::Friend if online => t("shell.online"),
        RowKind::Friend => t("shell.offline_short"),
        RowKind::Incoming => t("friends.incoming_hint"),
        RowKind::Outgoing => t("friends.outgoing_hint"),
        RowKind::Blocked => t("friends.blocked_hint"),
    };

    rsx! {
        div {
            class: "friend-row",
            onclick: move |_| {
                if kind == RowKind::Friend {
                    state.message_users(vec![id]);
                }
            },
            onmousedown: move |e| {
                if let Some((x, y)) = menu::right_click(&e) {
                    menu::open(state, x, y, menu::user_items(state, id, None, None));
                }
            },
            div { class: "member-avatar",
                Avatar { name: name.clone(), id: Some(id), size: 36 }
                if kind == RowKind::Friend {
                    div { class: if online { "presence-dot online" } else { "presence-dot" } }
                }
            }
            div { class: "friend-text",
                div { class: "friend-name", "{name} " span { class: "muted small", "@{user.username}" } }
                div { class: "muted small", "{status}" }
            }
            div { class: "friend-actions", onclick: move |e| e.stop_propagation(),
                match kind {
                    RowKind::Friend => rsx! {
                        IconButton { icon: Icon::Message, size: 18, class: "round", title: t("menu.message"), onclick: move |_| state.message_users(vec![id]) }
                        IconButton { icon: Icon::Phone, size: 18, class: "round", title: t("call.start"), onclick: move |_| call_user(state, id) }
                        IconButton { icon: Icon::UserMinus, size: 18, class: "round danger-hover", title: t("menu.remove_friend"),
                            onclick: move |_| act(state, async move |s| s.api.remove_friend(id).await) }
                    },
                    RowKind::Incoming => rsx! {
                        IconButton { icon: Icon::Check, size: 18, class: "round accept", title: t("friends.accept"),
                            onclick: move |_| act(state, async move |s| s.api.accept_friend(id).await) }
                        IconButton { icon: Icon::X, size: 18, class: "round danger-hover", title: t("friends.decline"),
                            onclick: move |_| act(state, async move |s| s.api.cancel_friend_request(id).await) }
                    },
                    RowKind::Outgoing => rsx! {
                        IconButton { icon: Icon::X, size: 18, class: "round danger-hover", title: t("friends.cancel_request"),
                            onclick: move |_| act(state, async move |s| s.api.cancel_friend_request(id).await) }
                    },
                    RowKind::Blocked => rsx! {
                        button { class: "button small", onclick: move |_| act(state, async move |s| s.api.unblock(id).await), {t("menu.unblock")} }
                    },
                }
            }
        }
    }
}

/// Opens the DM with a friend and calls them.
fn call_user(state: AppState, user: UserId) {
    let Some(s) = state.session.cloned() else { return };
    spawn_forever(async move {
        match s.api.open_dm(vec![user]).await {
            Ok(dm) => {
                let id = dm.channel.id;
                state.add_dm(dm);
                state.open_channel(id);
                super::call::start(state, id, false);
            }
            Err(e) => state.report(Err(e.to_string())),
        }
    });
}

#[component]
fn AddFriend() -> Element {
    let state = use_context::<AppState>();
    let mut name = use_signal(String::new);
    let mut result = use_signal(|| None::<(bool, String)>);
    let mut busy = use_signal(|| false);
    let mut send = move || {
        let value = name.read().trim().trim_start_matches('@').to_owned();
        let Some(s) = state.session.cloned() else { return };
        if value.is_empty() || *busy.peek() {
            return;
        }
        busy.set(true);
        spawn(async move {
            let r = s.api.send_friend_request(&value).await;
            busy.set(false);
            match r {
                Ok(()) => {
                    result.set(Some((true, format!("{} @{value}", t("friends.request_sent_to")))));
                    name.set(String::new());
                    state.report(state.refresh_friends().await);
                }
                Err(e) => result.set(Some((false, e.to_string()))),
            }
        });
    };

    rsx! {
        div { class: "add-friend",
            h2 { {t("friends.add_title")} }
            p { class: "muted", {t("friends.add_hint")} }
            div { class: "add-friend-box",
                input {
                    autofocus: "true",
                    value: "{name}",
                    placeholder: t("friends.add_placeholder"),
                    oninput: move |e| {
                        name.set(e.value());
                        result.set(None);
                    },
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            send();
                        }
                    },
                }
                button { class: if *busy.read() { "button primary busy" } else { "button primary" }, onclick: move |_| send(), {t("friends.send_request")} }
            }
            if let Some((ok, text)) = result.read().clone() {
                div { class: if ok { "form-ok" } else { "form-error" }, "{text}" }
            }
        }
    }
}

/// Picks friends for a new DM or group (`channel: None`), or people to add
/// to an existing one.
#[component]
pub fn PickPeople(channel: Option<ChannelId>) -> Element {
    let mut state = use_context::<AppState>();
    let mut chosen = use_signal(HashSet::<UserId>::new);
    let mut filter = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let existing: HashSet<UserId> =
        channel.and_then(|c| state.dm(c)).map(|d| d.members.iter().map(|u| u.id).collect()).unwrap_or_default();
    let max_new = 10usize.saturating_sub(existing.len().max(1));
    let q = filter.read().to_lowercase();
    let mut friends: Vec<User> = state
        .friends
        .read()
        .friends
        .iter()
        .filter(|u| !existing.contains(&u.id))
        .filter(|u| q.is_empty() || display_name(u).to_lowercase().contains(&q) || u.username.contains(&q))
        .cloned()
        .collect();
    friends.sort_by_key(|u| display_name(u).to_lowercase());
    let count = chosen.read().len();

    let confirm = move |_| {
        let Some(s) = state.session.cloned() else { return };
        let users: Vec<UserId> = chosen.read().iter().copied().collect();
        if users.is_empty() || *busy.peek() {
            return;
        }
        busy.set(true);
        spawn_forever(async move {
            let result = match channel.and_then(|c| state.dm(c)) {
                // A group grows; adding to a 1:1 DM makes a new group with everyone.
                Some(dm) if dm.channel.kind == ChannelKind::Group => {
                    let mut last = Ok(dm.clone());
                    for u in users {
                        last = s.api.add_dm_member(dm.channel.id, u).await;
                        if last.is_err() {
                            break;
                        }
                    }
                    last
                }
                Some(dm) => {
                    let mut all: Vec<UserId> = dm.members.iter().map(|u| u.id).filter(|u| *u != s.user.id).collect();
                    all.extend(users);
                    s.api.open_dm(all).await
                }
                None => s.api.open_dm(users).await,
            };
            busy.set(false);
            match result {
                Ok(dm) => {
                    let id = dm.channel.id;
                    state.add_dm(dm);
                    state.modal.set(None);
                    state.open_channel(id);
                }
                Err(e) => state.report(Err(e.to_string())),
            }
        });
    };

    rsx! {
        div { class: "overlay", onclick: move |_| state.modal.set(None),
            div { class: "dialog", onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { if channel.is_some() { {t("dm.add_people")} } else { {t("home.new_dm")} } }
                    IconButton { icon: Icon::X, size: 18, onclick: move |_| state.modal.set(None) }
                }
                p { class: "muted", "{t(\"dm.pick_hint\")} ({count}/{max_new})" }
                input {
                    class: "input",
                    autofocus: "true",
                    value: "{filter}",
                    placeholder: t("dm.search_friends"),
                    oninput: move |e| filter.set(e.value()),
                }
                div { class: "pick-list",
                    if friends.is_empty() {
                        div { class: "muted small", {t("dm.no_friends")} }
                    }
                    for u in friends {
                        {
                            let id = u.id;
                            let on = chosen.read().contains(&id);
                            let name = display_name(&u);
                            rsx! {
                                div {
                                    key: "{id}",
                                    class: if on { "pick-row on" } else { "pick-row" },
                                    onclick: move |_| {
                                        let mut c = chosen.write();
                                        if !c.remove(&id) && c.len() < max_new {
                                            c.insert(id);
                                        }
                                    },
                                    Avatar { name: name.clone(), id: Some(id), size: 30 }
                                    span { class: "pick-name", "{name}" }
                                    span { class: "muted small", "@{u.username}" }
                                    span { class: "topbar-spacer" }
                                    div { class: if on { "menu-check on" } else { "menu-check" },
                                        if on { Svg { icon: Icon::Check, size: 14 } }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "dialog-actions",
                    button { class: "button ghost", onclick: move |_| state.modal.set(None), {t("common.cancel")} }
                    button { class: if *busy.read() || count == 0 { "button primary busy" } else { "button primary" }, onclick: confirm,
                        if channel.is_some() { {t("dm.add")} } else { {t("home.create_dm")} }
                    }
                }
            }
        }
    }
}
