//! Server settings: name, icon, AFK channel and timeout, bans.

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::GuildId;
use sivryn_client_core::protocol::api::{Ban, UpdateGuildRequest};
use sivryn_client_core::protocol::model::{AFK_TIMEOUTS, AUDIO_BITRATES, ChannelKind};
use sivryn_client_core::protocol::permissions::Permissions;

use super::AppState;
use super::icons::{Icon, Svg};
use super::widgets::{Avatar, IconButton, Select, opt};
use crate::i18n::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Overview,
    Roles,
    Bans,
}

#[component]
pub fn GuildSettings(guild: GuildId) -> Element {
    let mut state = use_context::<AppState>();
    let perms = state.permissions(guild);
    let can_manage = perms.allows(Permissions::MANAGE_GUILD);
    let can_ban = perms.allows(Permissions::BAN_MEMBERS);
    let can_roles = perms.allows(Permissions::MANAGE_ROLES);
    let mut page = use_signal(move || {
        if can_manage {
            Page::Overview
        } else if can_roles {
            Page::Roles
        } else {
            Page::Bans
        }
    });
    let Some(g) = state.guild(guild) else { return rsx! {} };
    let current = *page.read();

    rsx! {
        div { class: "settings",
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    state.modal.set(None);
                }
            },
            nav { class: "settings-nav",
                div { class: "settings-nav-title", "{g.name}" }
                if can_manage {
                    div { class: if current == Page::Overview { "nav-item active" } else { "nav-item" }, onclick: move |_| page.set(Page::Overview),
                        Svg { icon: Icon::Settings, size: 18 }
                        span { {t("guild.overview")} }
                    }
                }
                if can_roles {
                    div { class: if current == Page::Roles { "nav-item active" } else { "nav-item" }, onclick: move |_| page.set(Page::Roles),
                        Svg { icon: Icon::Shield, size: 18 }
                        span { {t("roles.title")} }
                    }
                }
                if can_ban {
                    div { class: if current == Page::Bans { "nav-item active" } else { "nav-item" }, onclick: move |_| page.set(Page::Bans),
                        Svg { icon: Icon::Ban, size: 18 }
                        span { {t("guild.bans")} }
                    }
                }
            }
            // Keyed per page, so switching pages starts at the top.
            for p in [current] {
                div { key: "{p:?}", class: "settings-body",
                    div { class: "settings-content",
                        match p {
                            Page::Overview => rsx! { Overview { guild } },
                            Page::Roles => rsx! { super::roles::RolesPage { guild } },
                            Page::Bans => rsx! { Bans { guild } },
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

#[component]
fn Overview(guild: GuildId) -> Element {
    let state = use_context::<AppState>();
    let Some(g) = state.guild(guild) else { return rsx! {} };
    let mut name = use_signal(|| g.name.clone());
    let saving = use_signal(|| false);
    let voice: Vec<(String, String)> = state
        .guilds
        .read()
        .iter()
        .find(|x| x.guild.id == guild)
        .map(|x| {
            x.channels
                .iter()
                .filter(|c| c.kind == ChannelKind::Voice)
                .map(|c| (c.id.to_string(), c.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    let changed = name.read().trim() != g.name && !name.read().trim().is_empty();

    let update = move |req: UpdateGuildRequest| {
        let Some(s) = state.session.cloned() else { return };
        let mut saving = saving;
        saving.set(true);
        spawn_forever(async move {
            let r = s.api.update_guild(guild, &req).await.map(|_| ()).map_err(|e| e.to_string());
            super::widgets::set_if_alive(saving, false);
            state.report(r);
            state.report(state.refresh().await);
        });
    };

    let mut afk_options = vec![opt("", t("guild.afk_none"))];
    afk_options.extend(voice.iter().map(|(id, name)| opt(id.clone(), name.clone())));
    let timeout_options: Vec<_> = AFK_TIMEOUTS
        .iter()
        .map(|m| opt(m.to_string(), format!("{m} {}", if *m == 1 { t("guild.minute") } else { t("guild.minutes") })))
        .collect();

    rsx! {
        h1 { class: "section-title", {t("guild.overview")} }
        div { class: "guild-overview",
            GuildIconEditor { guild }
            div { class: "field grow",
                div { class: "field-label", {t("guild.name")} }
                div { class: "row",
                    input {
                        class: "input",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter && changed {
                                update(UpdateGuildRequest { name: Some(name.peek().trim().to_owned()), ..Default::default() });
                            }
                        },
                    }
                    if changed {
                        button {
                            class: if *saving.read() { "button primary busy" } else { "button primary" },
                            onclick: move |_| update(UpdateGuildRequest { name: Some(name.peek().trim().to_owned()), ..Default::default() }),
                            {t("common.save")}
                        }
                    }
                }
            }
        }
        div { class: "field",
            div { class: "field-label", {t("guild.afk_channel")} }
            Select {
                value: g.afk_channel_id.map(|c| c.to_string()).unwrap_or_default(),
                options: afk_options,
                onchange: move |v: String| {
                    let channel = v.parse().ok();
                    update(UpdateGuildRequest { afk_channel_id: Some(channel), ..Default::default() });
                },
            }
            div { class: "hint", {t("guild.afk_channel_hint")} }
        }
        div { class: "field",
            div { class: "field-label", {t("guild.afk_timeout")} }
            Select {
                value: g.afk_timeout_minutes.to_string(),
                options: timeout_options,
                onchange: move |v: String| {
                    if let Ok(m) = v.parse() {
                        update(UpdateGuildRequest { afk_timeout_minutes: Some(m), ..Default::default() });
                    }
                },
            }
        }
        div { class: "field",
            div { class: "field-label", {t("guild.audio_bitrate")} }
            Select {
                value: g.audio_bitrate.to_string(),
                options: AUDIO_BITRATES.iter().map(|b| opt(b.to_string(), format!("{} kbit/s", b / 1000))).collect(),
                onchange: move |v: String| {
                    if let Ok(b) = v.parse() {
                        update(UpdateGuildRequest { audio_bitrate: Some(b), ..Default::default() });
                    }
                },
            }
            div { class: "hint", {t("guild.audio_bitrate_hint")} }
        }
        if Some(g.owner_id) == state.me() {
            div { class: "divider" }
            DeleteGuild { guild, name: g.name.clone() }
        }
    }
}

/// Deleting the server, after typing its name.
#[component]
fn DeleteGuild(guild: GuildId, name: String) -> Element {
    let mut state = use_context::<AppState>();
    let mut typed = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let matches = typed.read().trim() == name.trim();
    let delete = move |_| {
        let Some(s) = state.session.cloned() else { return };
        if !matches || *busy.peek() {
            return;
        }
        busy.set(true);
        spawn_forever(async move {
            match s.api.delete_guild(guild).await {
                Ok(()) => {
                    state.modal.set(None);
                    state.guild.set(None);
                    state.channel.set(None);
                    state.home.set(true);
                    state.report(state.refresh().await);
                }
                Err(e) => {
                    super::widgets::set_if_alive(busy, false);
                    state.report(Err(e.to_string()));
                }
            }
        });
    };
    rsx! {
        div { class: "danger-zone",
            h2 { class: "subsection", {t("guild.delete")} }
            p { class: "muted small", {t("guild.delete_hint")} }
            div { class: "row",
                input {
                    class: "input",
                    value: "{typed}",
                    placeholder: "{name}",
                    oninput: move |e| typed.set(e.value()),
                }
                button { class: if matches && !*busy.read() { "button danger" } else { "button danger busy" }, onclick: delete,
                    {t("guild.delete_action")}
                }
            }
        }
    }
}

#[component]
fn GuildIconEditor(guild: GuildId) -> Element {
    let state = use_context::<AppState>();
    let busy = use_signal(|| false);
    let g = state.guild(guild);
    let name = g.as_ref().map(|g| g.name.clone()).unwrap_or_default();
    let icon = g.and_then(|g| g.icon_id);

    // Pick an image and crop it to a 256 px square (GIFs stay animated).
    let pick = move |_| super::crop::pick(state, super::crop::CropPurpose::GuildIcon(guild));

    rsx! {
        div { class: "icon-editor",
            div { class: "icon-editor-preview", onclick: pick,
                GuildIcon { name: name.clone(), icon, size: 84 }
                div { class: "icon-editor-overlay", Svg { icon: Icon::Upload, size: 22 } }
            }
            button { class: if *busy.read() { "button small busy" } else { "button small" }, onclick: pick, {t("guild.change_icon")} }
        }
    }
}

/// A guild's icon, or its initials while there is none.
#[component]
pub fn GuildIcon(
    name: String,
    icon: Option<sivryn_client_core::protocol::FileId>,
    #[props(default = 48)] size: u32,
) -> Element {
    let state = use_context::<AppState>();
    // Animated icons play only while hovered (each frame repaints the window).
    let (hovered, mark) = super::widgets::use_hover();
    let gate = use_hook(move || std::rc::Rc::new(tokio::sync::watch::channel(false)));
    if *gate.0.borrow() != hovered {
        let _ = gate.0.send(hovered);
    }
    let gate_rx = gate.1.clone();
    // Reactive on `icon`: a new icon must reload even in the same component.
    let frames = use_resource(use_reactive!(|icon, size| {
        let gate_rx = gate_rx.clone();
        async move {
            let s = state.session.cloned()?;
            let id = icon?;
            let bytes = match crate::files::fetch_plain(&s, id).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, "server icon unavailable");
                    return None;
                }
            };
            let decoded =
                tokio::task::spawn_blocking(move || crate::files::decode(&bytes, size * 2, size * 2)).await.ok()?;
            match decoded {
                Ok(d) => Some((id, crate::files::play_gated(d, gate_rx))),
                Err(e) => {
                    tracing::warn!(error = %e, "server icon unreadable");
                    None
                }
            }
        }
    }));
    let initials: String =
        name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase();
    match frames.read().clone().flatten() {
        Some((id, frames)) => rsx! {
            div { class: "guild-icon", style: "width: {size}px; height: {size}px;", onmousemove: mark,
                // Keyed by the loaded icon (keys only work in lists), so a new
                // icon gets a new canvas once it has loaded.
                for id in [id] {
                    super::media::FrameCanvas { key: "{id}", frames: super::media::Frames(frames.clone()) }
                }
            }
        },
        None => rsx! { "{initials}" },
    }
}

#[component]
fn Bans(guild: GuildId) -> Element {
    let state = use_context::<AppState>();
    let mut bans = use_signal(|| None::<Vec<Ban>>);
    let mut reload = use_signal(|| 0u32);
    use_effect(move || {
        let _ = reload.read();
        let Some(s) = state.session.cloned() else { return };
        spawn(async move {
            match s.api.bans(guild).await {
                Ok(list) => bans.set(Some(list)),
                Err(e) => state.report(Err(e.to_string())),
            }
        });
    });

    rsx! {
        h1 { class: "section-title", {t("guild.bans")} }
        p { class: "muted", {t("guild.bans_hint")} }
        match bans.read().clone() {
            None => rsx! { div { class: "spinner" } },
            Some(list) if list.is_empty() => rsx! { div { class: "muted", {t("guild.no_bans")} } },
            Some(list) => rsx! {
                div { class: "user-rows",
                    for ban in list {
                        div { key: "{ban.user.id}", class: "user-row",
                            Avatar { name: ban.user.display_name.clone().unwrap_or_else(|| ban.user.username.clone()), id: Some(ban.user.id), size: 32 }
                            span { class: "user-row-name", "{ban.user.display_name.clone().unwrap_or_else(|| ban.user.username.clone())}" }
                            span { class: "muted small", "@{ban.user.username}" }
                            if let Some(reason) = ban.reason.clone() {
                                span { class: "muted small", "– {reason}" }
                            }
                            span { class: "topbar-spacer" }
                            button {
                                class: "button small",
                                onclick: {
                                    let user = ban.user.id;
                                    move |_| {
                                        let Some(s) = state.session.cloned() else { return };
                                        spawn(async move {
                                            state.report(s.api.unban(guild, user).await.map_err(|e| e.to_string()));
                                            reload += 1;
                                        });
                                    }
                                },
                                {t("guild.unban")}
                            }
                        }
                    }
                }
            },
        }
    }
}
