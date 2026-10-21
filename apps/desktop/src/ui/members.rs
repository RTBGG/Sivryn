//! Member list on the right: online members grouped by their highest role
//! that is listed separately, then the other online members, then offline
//! ones; alphabetical, collapsible, names in their role's color.

use std::collections::HashSet;

use dioxus_native::prelude::*;
use sivryn_client_core::protocol::model::UserStatus;
use sivryn_client_core::protocol::{ChannelId, UserId};

use super::icons::{Icon, Svg};
use super::roles::hex;
use super::widgets::{Avatar, StatusDot};
use super::{AppState, Modal, menu};
use crate::i18n::t;

#[derive(Clone, PartialEq)]
struct Row {
    id: UserId,
    name: String,
    status: Option<UserStatus>,
    color: Option<u32>,
}

#[derive(Clone, PartialEq)]
struct Group {
    key: String,
    title: String,
    rows: Vec<Row>,
}

#[component]
pub fn MemberList(channel: ChannelId) -> Element {
    let state = use_context::<AppState>();
    let mut collapsed = use_signal(HashSet::<String>::new);
    let guild = state.guild_of(channel);
    let ids: Vec<UserId> = match guild {
        Some(g) => state.members.read().get(&g).map(|l| l.iter().map(|m| m.user.id).collect()).unwrap_or_default(),
        None => state.dm(channel).map(|d| d.members.iter().map(|u| u.id).collect()).unwrap_or_default(),
    };
    let owner = guild.and_then(|g| state.guild(g)).map(|g| g.owner_id);
    let roles = guild.map(|g| state.roles(g)).unwrap_or_default();
    let mut rows: Vec<Row> = ids
        .into_iter()
        .map(|id| Row {
            id,
            name: state.display_name_in(guild, id),
            status: state.status_of(id),
            color: state.role_color(guild, id),
        })
        .collect();
    rows.sort_by_key(|r| r.name.to_lowercase());
    let (online, offline): (Vec<Row>, Vec<Row>) =
        rows.into_iter().partition(|r| r.status.is_some_and(|s| s != UserStatus::Invisible));

    // Online members under their highest separately listed role (roles are
    // highest first), the rest under "Online".
    let mut groups: Vec<Group> = Vec::new();
    let mut rest: Vec<Row> = Vec::new();
    for row in online {
        let mine = guild.map(|g| state.member_roles(g, row.id)).unwrap_or_default();
        match roles.iter().find(|r| r.hoist && mine.contains(&r.id)) {
            Some(role) => match groups.iter_mut().find(|g| g.key == role.id.to_string()) {
                Some(group) => group.rows.push(row),
                None => groups.push(Group { key: role.id.to_string(), title: role.name.clone(), rows: vec![row] }),
            },
            None => rest.push(row),
        }
    }
    groups.sort_by_key(|g| roles.iter().position(|r| r.id.to_string() == g.key));
    groups.push(Group { key: "online".into(), title: t("members.online").into(), rows: rest });
    groups.push(Group { key: "offline".into(), title: t("members.offline").into(), rows: offline });
    groups.retain(|g| !g.rows.is_empty() || g.key == "online");

    rsx! {
        aside { class: "member-list",
            for group in groups {
                {
                    let key = group.key.clone();
                    let open = !collapsed.read().contains(&key);
                    let toggle_key = key.clone();
                    let offline = key == "offline";
                    rsx! {
                        div { key: "{key}",
                            div {
                                class: "member-section",
                                onclick: move |_| {
                                    let mut c = collapsed.write();
                                    if !c.remove(&toggle_key) {
                                        c.insert(toggle_key.clone());
                                    }
                                },
                                if open { Svg { icon: Icon::ChevronDown, size: 14 } } else { Svg { icon: Icon::ChevronRight, size: 14 } }
                                "{group.title} — {group.rows.len()}"
                            }
                            if open {
                                for r in group.rows {
                                    MemberRow { key: "{r.id}", row: r.clone(), owner: owner == Some(r.id), channel, offline }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

thread_local! {
    /// Pointer moves over a crown, to hide its tooltip once they stop.
    static TIP_MOVES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Set by a crown's mouse move for the row handler that sees it next.
    static ON_CROWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[component]
fn MemberRow(row: Row, owner: bool, channel: ChannelId, offline: bool) -> Element {
    let mut state = use_context::<AppState>();
    let id = row.id;
    // Shown while the pointer is on the crown: set by its own mousemove,
    // cleared by the row's (CSS :hover didn't trigger on the small icon).
    let mut crown_tip = use_signal(|| false);
    // An animated avatar plays while the row is hovered.
    let (row_hovered, mut mark_row) = super::widgets::use_hover();
    let style = row.color.map(|c| format!("color: {};", hex(c))).unwrap_or_default();
    rsx! {
        div {
            class: if offline { "member offline" } else { "member" },
            onclick: move |_| state.modal.set(Some(Modal::Profile(id))),
            onmousemove: move |e| {
                mark_row(e);
                // The crown's own move (which bubbles here) keeps its tooltip.
                if !ON_CROWN.with(|c| c.replace(false)) && *crown_tip.peek() {
                    crown_tip.set(false);
                }
            },
            onmouseleave: move |_| crown_tip.set(false),
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
                Avatar { name: row.name.clone(), id: Some(id), size: 32, animate: row_hovered }
                StatusDot { status: row.status.filter(|_| !offline) }
            }
            span { class: "member-name", style: "{style}", "{row.name}" }
            if owner {
                span {
                    class: "owner-badge has-tip",
                    onmousemove: move |_| {
                        ON_CROWN.with(|c| c.set(true));
                        // Each move restarts a short timer; leaving the list
                        // may not send any event that would hide it.
                        let generation = TIP_MOVES.with(|g| {
                            g.set(g.get() + 1);
                            g.get()
                        });
                        if !*crown_tip.peek() {
                            crown_tip.set(true);
                        }
                        spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
                            if TIP_MOVES.with(|g| g.get()) == generation {
                                crown_tip.set(false);
                            }
                        });
                    },
                    Svg { icon: Icon::Crown, size: 14 }
                    if *crown_tip.read() {
                        span { class: "tip shown", {t("members.owner")} }
                    }
                }
            }
        }
    }
}
