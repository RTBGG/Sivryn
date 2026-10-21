//! Settings of a channel or a category: name, deleting, and permission
//! overwrites for roles and members (private channels, read-only channels…).

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::api::UpdateChannelRequest;
use sivryn_client_core::protocol::model::{ChannelKind, OverwriteTarget, PermissionOverwrite};
use sivryn_client_core::protocol::permissions::Permissions;
use sivryn_client_core::protocol::{CategoryId, ChannelId, GuildId};

use super::AppState;
use super::icons::{Icon, Svg};
use super::roles::{hex, perm_text};
use super::widgets::{Avatar, IconButton, Select, Toggle, opt};
use crate::i18n::t;

/// A channel or a category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Channel(ChannelId),
    Category(CategoryId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Overview,
    Permissions,
}

/// What the settings show about a place, read from the loaded guilds.
struct Info {
    guild: GuildId,
    name: String,
    kind: Option<ChannelKind>,
    overwrites: Vec<PermissionOverwrite>,
}

fn info(state: &AppState, place: Place) -> Option<Info> {
    let guilds = state.guilds.read();
    match place {
        Place::Channel(id) => guilds.iter().find_map(|g| {
            g.channels.iter().find(|c| c.id == id).map(|c| Info {
                guild: g.guild.id,
                name: c.name.clone(),
                kind: Some(c.kind),
                overwrites: c.overwrites.clone(),
            })
        }),
        Place::Category(id) => guilds.iter().find_map(|g| {
            g.categories.iter().find(|c| c.id == id).map(|c| Info {
                guild: g.guild.id,
                name: c.name.clone(),
                kind: None,
                overwrites: c.overwrites.clone(),
            })
        }),
    }
}

/// The permissions an overwrite can change here.
fn place_permissions(kind: Option<ChannelKind>) -> Vec<Permissions> {
    let text = [Permissions::SEND_MESSAGES, Permissions::ATTACH_FILES, Permissions::MANAGE_MESSAGES];
    let voice = [Permissions::CONNECT, Permissions::SPEAK, Permissions::VIDEO, Permissions::SCREEN_SHARE];
    let mut out = vec![Permissions::VIEW_CHANNEL];
    match kind {
        Some(ChannelKind::Text) => out.extend(text),
        // Voice channels have their own chat.
        Some(ChannelKind::Voice) => {
            out.extend(voice);
            out.extend(text);
        }
        _ => {
            out.extend(text);
            out.extend(voice);
        }
    }
    out
}

#[component]
pub fn PlaceSettings(place: Place) -> Element {
    let mut state = use_context::<AppState>();
    let Some(info) = info(&state, place) else { return rsx! {} };
    let can_roles = state.permissions(info.guild).allows(Permissions::MANAGE_ROLES);
    let mut page = use_signal(|| Page::Overview);
    let current = *page.read();
    let title = match place {
        Place::Channel(_) => t("channels.settings"),
        Place::Category(_) => t("channels.edit_category"),
    };

    rsx! {
        div { class: "settings",
            nav { class: "settings-nav",
                div { class: "settings-nav-title", "{info.name}" }
                div { class: if current == Page::Overview { "nav-item active" } else { "nav-item" }, onclick: move |_| page.set(Page::Overview),
                    Svg { icon: Icon::Settings, size: 18 }
                    span { {t("guild.overview")} }
                }
                if can_roles {
                    div { class: if current == Page::Permissions { "nav-item active" } else { "nav-item" }, onclick: move |_| page.set(Page::Permissions),
                        Svg { icon: Icon::Lock, size: 18 }
                        span { {t("perms.title")} }
                    }
                }
            }
            for p in [current] {
                div { key: "{p:?}", class: "settings-body",
                    div { class: "settings-content",
                        h1 { class: "section-title", "{title}" }
                        match p {
                            Page::Overview => rsx! { Overview { place } },
                            Page::Permissions => rsx! { Overwrites { place } },
                        }
                    }
                    div { class: "settings-close",
                        IconButton { icon: Icon::X, class: "outlined", onclick: move |_| state.modal.set(None) }
                        span { "ESC" }
                    }
                }
            }
        }
    }
}

/// Renaming and deleting.
#[component]
fn Overview(place: Place) -> Element {
    let mut state = use_context::<AppState>();
    let current = info(&state, place).map(|i| i.name).unwrap_or_default();
    let mut name = use_signal({
        let current = current.clone();
        move || current
    });
    let mut busy = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    let changed = name.read().trim() != current && !name.read().trim().is_empty();
    let (label, delete_label, delete_hint) = match place {
        Place::Channel(_) => (t("channels.name"), t("channels.delete"), t("channels.delete_hint")),
        Place::Category(_) => {
            (t("channels.category_name"), t("channels.delete_category"), t("channels.delete_category_hint"))
        }
    };

    let mut save = move || {
        let Some(s) = state.session.cloned() else { return };
        let value = name.peek().trim().to_owned();
        if value.is_empty() || *busy.peek() {
            return;
        }
        busy.set(true);
        spawn_forever(async move {
            let result = match place {
                Place::Channel(c) => s
                    .api
                    .update_channel(c, &UpdateChannelRequest { name: Some(value), ..Default::default() })
                    .await
                    .map(|_| ()),
                Place::Category(c) => s.api.rename_category(c, &value).await,
            };
            super::widgets::set_if_alive(busy, false);
            state.report(result.map_err(|e| e.to_string()));
            state.report(state.refresh().await);
        });
    };
    let delete = move |_| {
        let Some(s) = state.session.cloned() else { return };
        if !*confirm_delete.peek() {
            confirm_delete.set(true);
            return;
        }
        spawn_forever(async move {
            let result = match place {
                Place::Channel(c) => {
                    if *state.channel.peek() == Some(c) {
                        state.channel.set(None);
                    }
                    s.api.delete_channel(c).await
                }
                Place::Category(c) => s.api.delete_category(c).await,
            };
            state.report(result.map_err(|e| e.to_string()));
            state.modal.set(None);
            state.report(state.refresh().await);
        });
    };

    rsx! {
        div { class: "field",
            div { class: "field-label", "{label}" }
            div { class: "row",
                input {
                    class: "input",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter && changed {
                            save();
                        }
                    },
                }
                if changed {
                    button { class: if *busy.read() { "button primary busy" } else { "button primary" }, onclick: move |_| save(),
                        {t("common.save")}
                    }
                }
            }
        }
        div { class: "divider" }
        div { class: "danger-zone",
            h2 { class: "subsection", "{delete_label}" }
            p { class: "muted small", "{delete_hint}" }
            button { class: "button danger", onclick: delete,
                if *confirm_delete.read() { {t("channels.delete_confirm")} } else { "{delete_label}" }
            }
        }
    }
}

/// Overwrites for roles and members, each permission allowed, denied or
/// left to the role (and category) settings.
#[component]
fn Overwrites(place: Place) -> Element {
    let state = use_context::<AppState>();
    let Some(info) = info(&state, place) else { return rsx! {} };
    let guild = info.guild;
    let roles = state.roles(guild);
    let Some(everyone) = roles.iter().find(|r| r.is_everyone).map(|r| r.id) else { return rsx! {} };
    let mut selected = use_signal(move || OverwriteTarget::Role(everyone));
    // Targets added in this session that have no overwrite yet.
    let mut added = use_signal(Vec::<OverwriteTarget>::new);

    let mut targets: Vec<OverwriteTarget> = vec![OverwriteTarget::Role(everyone)];
    // Roles from highest to lowest, then members.
    for r in roles.iter().filter(|r| !r.is_everyone) {
        let t = OverwriteTarget::Role(r.id);
        if info.overwrites.iter().any(|o| o.target == t) || added.read().contains(&t) {
            targets.push(t);
        }
    }
    for o in info.overwrites.iter().filter(|o| matches!(o.target, OverwriteTarget::Member(_))) {
        targets.push(o.target);
    }
    for t in added.read().iter().filter(|t| matches!(t, OverwriteTarget::Member(_))) {
        if !targets.contains(t) {
            targets.push(*t);
        }
    }
    let current = *selected.read();
    let overwrite = info.overwrites.iter().find(|o| o.target == current).cloned().unwrap_or(PermissionOverwrite {
        target: current,
        allow: Permissions::empty(),
        deny: Permissions::empty(),
    });
    let private = info
        .overwrites
        .iter()
        .any(|o| o.target == OverwriteTarget::Role(everyone) && o.deny.contains(Permissions::VIEW_CHANNEL));

    let write = move |o: PermissionOverwrite| {
        let Some(s) = state.session.cloned() else { return };
        let (channel, category) = match place {
            Place::Channel(c) => (Some(c), None),
            Place::Category(c) => (None, Some(c)),
        };
        spawn_forever(async move {
            state.report(s.api.set_overwrite(channel, category, &o).await.map_err(|e| e.to_string()));
            state.report(state.refresh().await);
        });
    };
    let everyone_overwrite = info.overwrites.iter().find(|o| o.target == OverwriteTarget::Role(everyone)).cloned();
    let toggle_private = move |on: bool| {
        let mut o = everyone_overwrite.clone().unwrap_or(PermissionOverwrite {
            target: OverwriteTarget::Role(everyone),
            allow: Permissions::empty(),
            deny: Permissions::empty(),
        });
        if on {
            o.allow -= Permissions::VIEW_CHANNEL;
            o.deny |= Permissions::VIEW_CHANNEL;
        } else {
            o.deny -= Permissions::VIEW_CHANNEL;
        }
        write(o);
    };

    // Roles and members that can still be added.
    let mut add_options = vec![opt("", t("perms.add_target"))];
    for r in roles.iter().filter(|r| !r.is_everyone && !targets.contains(&OverwriteTarget::Role(r.id))) {
        add_options.push(opt(format!("role:{}", r.id), format!("@{}", r.name)));
    }
    let members = state.members.read().get(&guild).cloned().unwrap_or_default();
    for m in members.iter().filter(|m| !targets.contains(&OverwriteTarget::Member(m.user.id))) {
        add_options.push(opt(format!("member:{}", m.user.id), state.display_name_in(Some(guild), m.user.id)));
    }

    let label = |target: OverwriteTarget| -> (String, Option<u32>) {
        match target {
            OverwriteTarget::Role(id) => {
                let role = roles.iter().find(|r| r.id == id);
                (role.map(|r| r.name.clone()).unwrap_or_default(), role.and_then(|r| r.color))
            }
            OverwriteTarget::Member(id) => (state.display_name_in(Some(guild), id), None),
        }
    };
    let (current_name, _) = label(current);
    let perms = place_permissions(info.kind);
    let has_overwrite = !(overwrite.allow | overwrite.deny).is_empty();

    rsx! {
        div { class: "setting-row",
            div { class: "setting-text",
                div { class: "setting-title", Svg { icon: Icon::Lock, size: 16 } " " {t("perms.private")} }
                div { class: "hint",
                    match place {
                        Place::Channel(_) => t("perms.private_hint"),
                        Place::Category(_) => t("perms.private_category_hint"),
                    }
                }
            }
            Toggle { on: private, onchange: toggle_private }
        }
        div { class: "divider" }
        div { class: "roles-layout",
            div { class: "role-list",
                Select {
                    value: String::new(),
                    options: add_options,
                    onchange: move |v: String| {
                        let target = match v.split_once(':') {
                            Some(("role", id)) => id.parse().ok().map(OverwriteTarget::Role),
                            Some(("member", id)) => id.parse().ok().map(OverwriteTarget::Member),
                            _ => None,
                        };
                        if let Some(target) = target {
                            added.write().push(target);
                            selected.set(target);
                        }
                    },
                }
                for target in targets.iter().copied() {
                    {
                        let (name, color) = label(target);
                        let key = match target {
                            OverwriteTarget::Role(id) => format!("r{id}"),
                            OverwriteTarget::Member(id) => format!("m{id}"),
                        };
                        rsx! {
                            div {
                                key: "{key}",
                                class: if target == current { "role-item active" } else { "role-item" },
                                onclick: move |_| selected.set(target),
                                match target {
                                    OverwriteTarget::Role(_) => rsx! {
                                        span { class: "role-dot", style: "background: {color.map(hex).unwrap_or(\"#99aab5\".into())};" }
                                    },
                                    OverwriteTarget::Member(id) => rsx! { Avatar { name: name.clone(), id: Some(id), size: 20 } },
                                }
                                span { class: "role-item-name", "{name}" }
                            }
                        }
                    }
                }
            }
            div { class: "role-editor",
                div { class: "role-editor-head",
                    h2 { class: "subsection", "{current_name}" }
                    if has_overwrite && current != OverwriteTarget::Role(everyone) {
                        button {
                            class: "button ghost small",
                            onclick: move |_| {
                                write(PermissionOverwrite { target: current, allow: Permissions::empty(), deny: Permissions::empty() });
                                added.write().retain(|t| *t != current);
                                selected.set(OverwriteTarget::Role(everyone));
                            },
                            {t("perms.remove_target")}
                        }
                    }
                }
                p { class: "hint", {t("perms.tristate_hint")} }
                for p in perms {
                    {
                        let (name, hint) = perm_text(p);
                        let value = if overwrite.allow.contains(p) { 1 } else if overwrite.deny.contains(p) { -1 } else { 0 };
                        let base = overwrite.clone();
                        rsx! {
                            div { key: "{p.bits()}", class: "setting-row",
                                div { class: "setting-text",
                                    div { class: "setting-title", "{name}" }
                                    div { class: "hint", "{hint}" }
                                }
                                TriState {
                                    value,
                                    onchange: move |v: i8| {
                                        let mut o = base.clone();
                                        o.allow -= p;
                                        o.deny -= p;
                                        match v {
                                            1 => o.allow |= p,
                                            -1 => o.deny |= p,
                                            _ => {}
                                        }
                                        write(o);
                                    },
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Deny (✗), inherit (/) or allow (✓).
#[component]
fn TriState(value: i8, onchange: EventHandler<i8>) -> Element {
    rsx! {
        div { class: "tristate",
            div {
                class: if value == -1 { "tristate-option deny active" } else { "tristate-option deny" },
                title: t("perms.deny"),
                onclick: move |_| onchange.call(-1),
                // Separate branches: Blitz bakes an icon's color when it's built.
                if value == -1 { Svg { icon: Icon::X, size: 15 } } else { Svg { icon: Icon::X, size: 15 } }
            }
            div {
                class: if value == 0 { "tristate-option inherit active" } else { "tristate-option inherit" },
                title: t("perms.inherit"),
                onclick: move |_| onchange.call(0),
                "/"
            }
            div {
                class: if value == 1 { "tristate-option allow active" } else { "tristate-option allow" },
                title: t("perms.allow"),
                onclick: move |_| onchange.call(1),
                if value == 1 { Svg { icon: Icon::Check, size: 15 } } else { Svg { icon: Icon::Check, size: 15 } }
            }
        }
    }
}
