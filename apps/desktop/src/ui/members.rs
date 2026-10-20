//! Member list on the right: online and offline, alphabetical, collapsible.

use dioxus_native::prelude::*;
use sivryn_client_core::protocol::model::UserStatus;
use sivryn_client_core::protocol::{ChannelId, UserId};

use super::icons::{Icon, Svg};
use super::widgets::{Avatar, StatusDot};
use super::{AppState, Modal, menu};
use crate::i18n::t;

#[derive(Clone, PartialEq)]
struct Row {
    id: UserId,
    name: String,
    status: Option<UserStatus>,
}

#[component]
pub fn MemberList(channel: ChannelId) -> Element {
    let state = use_context::<AppState>();
    let mut online_open = use_signal(|| true);
    let mut offline_open = use_signal(|| true);
    let guild = state.guild_of(channel);
    let ids: Vec<UserId> = match guild {
        Some(g) => state.members.read().get(&g).map(|l| l.iter().map(|m| m.user.id).collect()).unwrap_or_default(),
        None => state.dm(channel).map(|d| d.members.iter().map(|u| u.id).collect()).unwrap_or_default(),
    };
    let owner = guild.and_then(|g| state.guild(g)).map(|g| g.owner_id);
    let mut rows: Vec<Row> = ids
        .into_iter()
        .map(|id| Row { id, name: state.display_name_in(guild, id), status: state.status_of(id) })
        .collect();
    rows.sort_by_key(|r| r.name.to_lowercase());
    let (online, offline): (Vec<Row>, Vec<Row>) =
        rows.into_iter().partition(|r| r.status.is_some_and(|s| s != UserStatus::Invisible));

    rsx! {
        aside { class: "member-list",
            div {
                class: "member-section",
                onclick: move |_| online_open.toggle(),
                if *online_open.read() { Svg { icon: Icon::ChevronDown, size: 14 } } else { Svg { icon: Icon::ChevronRight, size: 14 } }
                "{t(\"members.online\")} — {online.len()}"
            }
            if *online_open.read() {
                for r in online {
                    MemberRow { key: "{r.id}", row: r.clone(), owner: owner == Some(r.id), channel }
                }
            }
            div {
                class: "member-section",
                onclick: move |_| offline_open.toggle(),
                if *offline_open.read() { Svg { icon: Icon::ChevronDown, size: 14 } } else { Svg { icon: Icon::ChevronRight, size: 14 } }
                "{t(\"members.offline\")} — {offline.len()}"
            }
            if *offline_open.read() {
                for r in offline {
                    MemberRow { key: "{r.id}", row: r.clone(), owner: owner == Some(r.id), channel }
                }
            }
        }
    }
}

#[component]
fn MemberRow(row: Row, owner: bool, channel: ChannelId) -> Element {
    let mut state = use_context::<AppState>();
    let id = row.id;
    let online = row.status.is_some_and(|s| s != UserStatus::Invisible);
    rsx! {
        div {
            class: if online { "member" } else { "member offline" },
            onclick: move |_| state.modal.set(Some(Modal::Profile(id))),
            onmousedown: move |e| {
                if let Some((x, y)) = menu::right_click(&e) {
                    let voice = state
                        .call
                        .peek()
                        .as_ref()
                        .filter(|c| c.participants.iter().any(|p| p.identity == id.to_string()))
                        .map(|c| c.channel);
                    menu::open(state, x, y, menu::user_items(state, id, state.guild_of(channel), voice));
                }
            },
            div { class: "member-avatar",
                Avatar { name: row.name.clone(), id: Some(id), size: 32 }
                StatusDot { status: row.status.filter(|_| online) }
            }
            span { class: "member-name", "{row.name}" }
            if owner {
                span { class: "owner-badge", title: t("members.owner"), Svg { icon: Icon::ShieldCheck, size: 14 } }
            }
        }
    }
}
