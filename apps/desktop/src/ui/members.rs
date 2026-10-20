//! Member list on the right: online and offline, alphabetical, collapsible.

use dioxus_native::prelude::*;
use sivryn_client_core::protocol::ChannelId;
use sivryn_client_core::protocol::model::User;

use super::icons::{Icon, Svg};
use super::widgets::Avatar;
use super::{AppState, Modal, menu};
use crate::i18n::t;

fn display_name(u: &User) -> String {
    u.display_name.clone().unwrap_or_else(|| u.username.clone())
}

#[component]
pub fn MemberList(channel: ChannelId) -> Element {
    let state = use_context::<AppState>();
    let mut online_open = use_signal(|| true);
    let mut offline_open = use_signal(|| true);
    let guild = state.guild_of(channel);
    let members: Vec<User> = match guild {
        Some(g) => state.members.read().get(&g).cloned().unwrap_or_default(),
        None => state.dm(channel).map(|d| d.members).unwrap_or_default(),
    };
    let online_set = state.online.read();
    let owner = guild.and_then(|g| state.guild(g)).map(|g| g.owner_id);
    let sort = |mut list: Vec<User>| {
        list.sort_by_key(|u| display_name(u).to_lowercase());
        list
    };
    let (online, offline): (Vec<User>, Vec<User>) = members.into_iter().partition(|u| online_set.contains(&u.id));
    let (online, offline) = (sort(online), sort(offline));
    drop(online_set);

    rsx! {
        aside { class: "member-list",
            div {
                class: "member-section",
                onclick: move |_| online_open.toggle(),
                if *online_open.read() { Svg { icon: Icon::ChevronDown, size: 14 } } else { Svg { icon: Icon::ChevronRight, size: 14 } }
                "{t(\"members.online\")} — {online.len()}"
            }
            if *online_open.read() {
                for u in online {
                    MemberRow { key: "{u.id}", user: u.clone(), online: true, owner: owner == Some(u.id), channel }
                }
            }
            div {
                class: "member-section",
                onclick: move |_| offline_open.toggle(),
                if *offline_open.read() { Svg { icon: Icon::ChevronDown, size: 14 } } else { Svg { icon: Icon::ChevronRight, size: 14 } }
                "{t(\"members.offline\")} — {offline.len()}"
            }
            if *offline_open.read() {
                for u in offline {
                    MemberRow { key: "{u.id}", user: u.clone(), online: false, owner: owner == Some(u.id), channel }
                }
            }
        }
    }
}

#[component]
fn MemberRow(user: User, online: bool, owner: bool, channel: ChannelId) -> Element {
    let mut state = use_context::<AppState>();
    let name = display_name(&user);
    let id = user.id;
    rsx! {
        div {
            class: if online { "member" } else { "member offline" },
            onclick: move |_| state.modal.set(Some(Modal::Profile(id))),
            onmousedown: move |e| {
                if let Some((x, y)) = menu::right_click(&e) {
                    let voice = state.call.peek().as_ref().filter(|c| c.participants.iter().any(|p| p.identity == id.to_string())).map(|c| c.channel);
                    menu::open(state, x, y, menu::user_items(state, id, state.guild_of(channel), voice));
                }
            },
            div { class: "member-avatar",
                Avatar { name: name.clone(), id: Some(id), size: 32 }
                div { class: if online { "presence-dot online" } else { "presence-dot" } }
            }
            span { class: "member-name", "{name}" }
            if owner {
                span { class: "owner-badge", title: t("members.owner"), Svg { icon: Icon::ShieldCheck, size: 14 } }
            }
        }
    }
}
