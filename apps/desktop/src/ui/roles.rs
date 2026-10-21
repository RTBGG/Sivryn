//! Roles: the server settings page with the role list and editor (name,
//! color, separate listing, permissions, members), the color picker, and the
//! permission lists shared with channel and category overwrites.

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::api::RoleRequest;
use sivryn_client_core::protocol::model::Role;
use sivryn_client_core::protocol::permissions::Permissions;
use sivryn_client_core::protocol::{GuildId, RoleId, UserId};

use super::AppState;
use super::icons::{Icon, Svg};
use super::widgets::{Avatar, Segmented, Select, Toggle, opt};
use crate::i18n::t;

/// Guild permissions by section, as the role editor lists them.
pub const ROLE_SECTIONS: &[(&str, &[Permissions])] = &[
    (
        "perms.section_general",
        &[
            Permissions::ADMINISTRATOR,
            Permissions::MANAGE_GUILD,
            Permissions::MANAGE_ROLES,
            Permissions::MANAGE_CHANNELS,
            Permissions::CREATE_INVITE,
        ],
    ),
    ("perms.section_members", &[Permissions::KICK_MEMBERS, Permissions::BAN_MEMBERS, Permissions::MOVE_MEMBERS]),
    (
        "perms.section_text",
        &[
            Permissions::VIEW_CHANNEL,
            Permissions::SEND_MESSAGES,
            Permissions::ATTACH_FILES,
            Permissions::MANAGE_MESSAGES,
        ],
    ),
    ("perms.section_voice", &[Permissions::CONNECT, Permissions::SPEAK, Permissions::VIDEO, Permissions::SCREEN_SHARE]),
];

/// Name and explanation of a permission.
pub fn perm_text(p: Permissions) -> (&'static str, &'static str) {
    let (name, hint) = match p {
        Permissions::ADMINISTRATOR => ("perm.administrator", "perm.administrator_hint"),
        Permissions::MANAGE_GUILD => ("perm.manage_guild", "perm.manage_guild_hint"),
        Permissions::MANAGE_ROLES => ("perm.manage_roles", "perm.manage_roles_hint"),
        Permissions::MANAGE_CHANNELS => ("perm.manage_channels", "perm.manage_channels_hint"),
        Permissions::CREATE_INVITE => ("perm.create_invite", "perm.create_invite_hint"),
        Permissions::KICK_MEMBERS => ("perm.kick_members", "perm.kick_members_hint"),
        Permissions::BAN_MEMBERS => ("perm.ban_members", "perm.ban_members_hint"),
        Permissions::MOVE_MEMBERS => ("perm.move_members", "perm.move_members_hint"),
        Permissions::VIEW_CHANNEL => ("perm.view_channel", "perm.view_channel_hint"),
        Permissions::SEND_MESSAGES => ("perm.send_messages", "perm.send_messages_hint"),
        Permissions::ATTACH_FILES => ("perm.attach_files", "perm.attach_files_hint"),
        Permissions::MANAGE_MESSAGES => ("perm.manage_messages", "perm.manage_messages_hint"),
        Permissions::CONNECT => ("perm.connect", "perm.connect_hint"),
        Permissions::SPEAK => ("perm.speak", "perm.speak_hint"),
        Permissions::VIDEO => ("perm.video", "perm.video_hint"),
        Permissions::SCREEN_SHARE => ("perm.screen_share", "perm.screen_share_hint"),
        _ => ("perm.other", "perm.other"),
    };
    (t(name), t(hint))
}

/// `#rrggbb` for a color.
pub fn hex(color: u32) -> String {
    format!("#{:06x}", color & 0xff_ffff)
}

/// Parses `#rgb`, `#rrggbb` or the same without `#`.
pub fn parse_hex(text: &str) -> Option<u32> {
    let s = text.trim().trim_start_matches('#');
    match s.len() {
        6 => u32::from_str_radix(s, 16).ok(),
        3 => {
            let v = u32::from_str_radix(s, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            Some(((r * 17) << 16) | ((g * 17) << 8) | (b * 17))
        }
        _ => None,
    }
}

/// Hue (0–360), saturation and value (0–1) of a color.
fn to_hsv(color: u32) -> (f32, f32, f32) {
    let r = ((color >> 16) & 0xff) as f32 / 255.0;
    let g = ((color >> 8) & 0xff) as f32 / 255.0;
    let b = (color & 0xff) as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, if max == 0.0 { 0.0 } else { d / max }, max)
}

fn from_hsv(h: f32, s: f32, v: f32) -> u32 {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    byte(r) << 16 | byte(g) << 8 | byte(b)
}

/// Discord-like preset colors.
const PRESETS: [u32; 20] = [
    0x1abc9c, 0x2ecc71, 0x3498db, 0x9b59b6, 0xe91e63, 0xf1c40f, 0xe67e22, 0xe74c3c, 0x95a5a6, 0x607d8b, 0x11806a,
    0x1f8b4c, 0x206694, 0x71368a, 0xad1457, 0xc27c0e, 0xa84300, 0x992d22, 0x979c9f, 0x546e7a,
];

/// Columns and rows of the click zones over the saturation/brightness square.
const GRID: (u32, u32) = (32, 20);
const HUE_STEPS: u32 = 60;

/// A color picker: presets, a saturation/brightness square with a hue bar,
/// and a hex field. `None` is "no color".
#[component]
pub fn ColorPicker(value: Option<u32>, onchange: EventHandler<Option<u32>>) -> Element {
    let (h0, s0, v0) = to_hsv(value.unwrap_or(0x99aab5));
    // The hue is kept separately: it gets lost in grays and black.
    let mut hue = use_signal(move || h0);
    let mut dragging = use_signal(|| None::<&'static str>);
    let mut text = use_signal(move || value.map(hex).unwrap_or_default());
    // Follow outside changes (presets, another role).
    let shown = value.map(hex).unwrap_or_default();
    let mut last = use_signal(|| value);
    if *last.peek() != value {
        last.set(value);
        text.set(shown.clone());
        if let Some(c) = value {
            let (h, s, _) = to_hsv(c);
            if s > 0.0 {
                hue.set(h);
            }
        }
    }
    let h = *hue.read();
    let (_, s, v) = value.map(to_hsv).unwrap_or((h, s0, v0));
    let pure = hex(from_hsv(h, 1.0, 1.0));
    let pick_sv = move |col: u32, row: u32| {
        let s = col as f32 / (GRID.0 - 1) as f32;
        let v = 1.0 - row as f32 / (GRID.1 - 1) as f32;
        onchange.call(Some(from_hsv(*hue.peek(), s, v)));
    };
    let mut pick_hue = move |i: u32| {
        let h = i as f32 * 360.0 / HUE_STEPS as f32;
        hue.set(h);
        if let Some(c) = value {
            let (_, s, v) = to_hsv(c);
            onchange.call(Some(from_hsv(h, s.max(0.05), v.max(0.05))));
        }
    };
    let commit_text = move || {
        if let Some(c) = parse_hex(&text.peek()) {
            onchange.call(Some(c));
        } else if text.peek().trim().is_empty() {
            onchange.call(None);
        }
    };

    rsx! {
        div { class: "color-picker",
            onmouseup: move |_| dragging.set(None),
            onmouseleave: move |_| dragging.set(None),
            div { class: "color-presets",
                div {
                    class: if value.is_none() { "color-swatch none active" } else { "color-swatch none" },
                    title: t("roles.no_color"),
                    onclick: move |_| onchange.call(None),
                    Svg { icon: Icon::X, size: 14 }
                }
                for c in PRESETS {
                    div {
                        key: "{c}",
                        class: if value == Some(c) { "color-swatch active" } else { "color-swatch" },
                        style: "background: {hex(c)};",
                        onclick: move |_| onchange.call(Some(c)),
                    }
                }
            }
            div { class: "color-sv", style: "background: {pure};",
                div { class: "color-sv-white" }
                div { class: "color-sv-black" }
                if value.is_some() {
                    div {
                        class: "color-sv-marker",
                        style: "left: calc({s * 100.0}% - 7px); top: calc({(1.0 - v) * 100.0}% - 7px);",
                    }
                }
                div { class: "color-sv-hit",
                    for row in 0..GRID.1 {
                        div { key: "{row}", class: "color-sv-row",
                            for col in 0..GRID.0 {
                                div {
                                    key: "{col}",
                                    onmousedown: move |_| {
                                        dragging.set(Some("sv"));
                                        pick_sv(col, row);
                                    },
                                    onmousemove: move |_| {
                                        if *dragging.peek() == Some("sv") {
                                            pick_sv(col, row);
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }
            div { class: "color-hue",
                div { class: "color-hue-marker", style: "left: calc({h / 3.6}% - 4px);" }
                div { class: "color-hue-hit",
                    for i in 0..HUE_STEPS {
                        div {
                            key: "{i}",
                            onmousedown: move |_| {
                                dragging.set(Some("hue"));
                                pick_hue(i);
                            },
                            onmousemove: move |_| {
                                if *dragging.peek() == Some("hue") {
                                    pick_hue(i);
                                }
                            },
                        }
                    }
                }
            }
            div { class: "color-hex-row",
                div { class: "color-current", style: if let Some(c) = value { format!("background: {};", hex(c)) } else { String::new() } }
                input {
                    class: "input color-hex",
                    value: "{text}",
                    placeholder: "#99aab5",
                    oninput: move |e| text.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            commit_text();
                        }
                    },
                    onblur: move |_| commit_text(),
                }
            }
        }
    }
}

/// Unsaved changes to a role.
#[derive(Clone, Debug, PartialEq)]
struct Draft {
    name: String,
    color: Option<u32>,
    hoist: bool,
    permissions: Permissions,
}

impl Draft {
    fn of(role: &Role) -> Self {
        Draft { name: role.name.clone(), color: role.color, hoist: role.hoist, permissions: role.permissions }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Display,
    Permissions,
    Members,
}

/// Server settings → Roles.
#[component]
pub fn RolesPage(guild: GuildId) -> Element {
    let state = use_context::<AppState>();
    let roles = state.roles(guild);
    let mut selected = use_signal(|| None::<RoleId>);
    let mut busy = use_signal(|| false);
    let current = selected.read().and_then(|id| roles.iter().find(|r| r.id == id).cloned());
    // Roles below `@everyone` don't exist; everything else in order.
    let movable: Vec<RoleId> = roles.iter().filter(|r| !r.is_everyone).map(|r| r.id).collect();

    let create = move |_| {
        let Some(s) = state.session.cloned() else { return };
        if *busy.peek() {
            return;
        }
        busy.set(true);
        spawn_forever(async move {
            let req = RoleRequest { name: Some(t("roles.new_role").into()), ..Default::default() };
            match s.api.create_role(guild, &req).await {
                Ok(role) => {
                    state.report(state.refresh().await);
                    selected.set(Some(role.id));
                }
                Err(e) => state.report(Err(e.to_string())),
            }
            busy.set(false);
        });
    };
    let order_now = std::rc::Rc::new(movable.clone());
    let reorder = move |role: RoleId, up: bool| {
        let Some(s) = state.session.cloned() else { return };
        let mut order = (*order_now).clone();
        let Some(i) = order.iter().position(|r| *r == role) else { return };
        let j = if up { i.checked_sub(1) } else { Some(i + 1).filter(|j| *j < order.len()) };
        let Some(j) = j else { return };
        order.swap(i, j);
        spawn_forever(async move {
            state.report(s.api.reorder_roles(guild, order).await.map_err(|e| e.to_string()));
            state.report(state.refresh().await);
        });
    };

    let count_of = |role: &Role| -> usize {
        if role.is_everyone {
            return state.members.read().get(&guild).map_or(0, |l| l.len());
        }
        state.members.read().get(&guild).map_or(0, |l| l.iter().filter(|m| m.roles.contains(&role.id)).count())
    };

    // Servers from before roles send none, not even @everyone.
    if roles.is_empty() {
        return rsx! {
            h1 { class: "section-title", {t("roles.title")} }
            div { class: "callout",
                Svg { icon: Icon::Info, size: 18 }
                span { {t("roles.server_too_old")} }
            }
        };
    }

    rsx! {
        h1 { class: "section-title", {t("roles.title")} }
        p { class: "muted lead-tight", {t("roles.hint")} }
        div { class: "roles-layout",
            div { class: "role-list",
                button { class: if *busy.read() { "button primary small busy" } else { "button primary small" }, onclick: create,
                    Svg { icon: Icon::Plus, size: 15 }
                    {t("roles.create")}
                }
                for role in roles.iter().cloned() {
                    {
                        let id = role.id;
                        let manageable = state.outranks(guild, role.position);
                        let count = count_of(&role);
                        let first = movable.first() == Some(&id);
                        let last = movable.last() == Some(&id);
                        let (up, down) = (reorder.clone(), reorder.clone());
                        rsx! {
                            div {
                                key: "{id}",
                                class: if Some(id) == *selected.read() { "role-item active" } else { "role-item" },
                                onclick: move |_| selected.set(Some(id)),
                                span { class: "role-dot", style: "background: {role.color.map(hex).unwrap_or(\"#99aab5\".into())};" }
                                span { class: "role-item-name", "{role.name}" }
                                span { class: "muted small", "{count}" }
                                if manageable && !role.is_everyone {
                                    div { class: "role-move",
                                        if !first {
                                            div { class: "role-move-button", title: t("roles.move_up"),
                                                onclick: move |e| {
                                                    e.stop_propagation();
                                                    up(id, true);
                                                },
                                                Svg { icon: Icon::ArrowUp, size: 14 }
                                            }
                                        }
                                        if !last {
                                            div { class: "role-move-button", title: t("roles.move_down"),
                                                onclick: move |e| {
                                                    e.stop_propagation();
                                                    down(id, false);
                                                },
                                                Svg { icon: Icon::ArrowDown, size: 14 }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "role-editor",
                match current {
                    Some(role) => rsx! {
                        // Keyed by role: a new selection starts a fresh draft.
                        for role in [role] {
                            RoleEditor { key: "{role.id}", guild, role, on_deleted: move |_| selected.set(None) }
                        }
                    },
                    None => rsx! { div { class: "muted role-empty", {t("roles.pick")} } },
                }
            }
        }
    }
}

#[component]
fn RoleEditor(guild: GuildId, role: Role, on_deleted: EventHandler<()>) -> Element {
    let state = use_context::<AppState>();
    let original = Draft::of(&role);
    let mut draft = use_signal({
        let original = original.clone();
        move || original
    });
    let mut tab = use_signal(move || if role.is_everyone { Tab::Permissions } else { Tab::Display });
    let mut busy = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    let editable = state.outranks(guild, role.position);
    let dirty = *draft.read() != original;
    let id = role.id;
    let everyone = role.is_everyone;

    let save = move |_| {
        let Some(s) = state.session.cloned() else { return };
        let d = draft.peek().clone();
        busy.set(true);
        spawn_forever(async move {
            let req = if everyone {
                RoleRequest { permissions: Some(d.permissions), ..Default::default() }
            } else {
                RoleRequest {
                    name: Some(d.name.trim().to_owned()),
                    permissions: Some(d.permissions),
                    color: Some(d.color),
                    hoist: Some(d.hoist),
                }
            };
            let result = s.api.update_role(id, &req).await.map(|_| ()).map_err(|e| e.to_string());
            busy.set(false);
            state.report(result);
            state.report(state.refresh().await);
        });
    };
    let reset = {
        let original = original.clone();
        move |_| draft.set(original.clone())
    };
    let delete = move |_| {
        let Some(s) = state.session.cloned() else { return };
        if !*confirm_delete.peek() {
            confirm_delete.set(true);
            return;
        }
        spawn_forever(async move {
            state.report(s.api.delete_role(id).await.map_err(|e| e.to_string()));
            state.report(state.refresh().await);
            on_deleted.call(());
        });
    };

    let mut tabs = Vec::new();
    if !everyone {
        tabs.push(opt("display", t("roles.tab_display")));
    }
    tabs.push(opt("permissions", t("roles.tab_permissions")));
    if !everyone {
        tabs.push(opt("members", t("roles.tab_members")));
    }
    let tab_value = match *tab.read() {
        Tab::Display => "display",
        Tab::Permissions => "permissions",
        Tab::Members => "members",
    };
    let d = draft.read().clone();

    rsx! {
        div { class: "role-editor-head",
            h2 { class: "subsection", "{role.name}" }
            Segmented {
                value: tab_value.to_string(),
                options: tabs,
                onchange: move |v: String| tab.set(match v.as_str() {
                    "display" => Tab::Display,
                    "members" => Tab::Members,
                    _ => Tab::Permissions,
                }),
            }
        }
        if !editable {
            div { class: "hint", {t("roles.too_high")} }
        }
        if dirty && editable {
            div { class: "save-bar",
                span { {t("roles.unsaved")} }
                span { class: "topbar-spacer" }
                button { class: "button ghost small", onclick: reset, {t("roles.reset")} }
                button { class: if *busy.read() { "button primary small busy" } else { "button primary small" }, onclick: save,
                    {t("common.save")}
                }
            }
        }
        match *tab.read() {
            Tab::Display => rsx! {
                div { class: "field",
                    div { class: "field-label", {t("roles.name")} }
                    input {
                        class: "input",
                        value: "{d.name}",
                        // Blitz treats `disabled: false` as disabled, so check here.
                        oninput: move |e| {
                            if editable {
                                draft.write().name = e.value();
                            }
                        },
                    }
                }
                div { class: "field",
                    div { class: "field-label", {t("roles.color")} }
                    div { class: "hint", {t("roles.color_hint")} }
                    ColorPicker { value: d.color, onchange: move |c| if editable { draft.write().color = c } }
                }
                div { class: "setting-row",
                    div { class: "setting-text",
                        div { class: "setting-title", {t("roles.hoist")} }
                        div { class: "hint", {t("roles.hoist_hint")} }
                    }
                    Toggle { on: d.hoist, onchange: move |v| if editable { draft.write().hoist = v } }
                }
                if editable {
                    div { class: "divider" }
                    button { class: "button danger-ghost", onclick: delete,
                        Svg { icon: Icon::Trash, size: 15 }
                        if *confirm_delete.read() { {t("roles.delete_confirm")} } else { {t("roles.delete")} }
                    }
                }
            },
            Tab::Permissions => rsx! {
                PermissionToggles {
                    value: d.permissions,
                    editable,
                    guild,
                    onchange: move |p| draft.write().permissions = p,
                }
            },
            Tab::Members => rsx! { RoleMembers { guild, role: id, editable } },
        }
    }
}

/// On/off switches for every guild permission, by section.
#[component]
fn PermissionToggles(
    value: Permissions,
    editable: bool,
    guild: GuildId,
    onchange: EventHandler<Permissions>,
) -> Element {
    let state = use_context::<AppState>();
    // Nobody can hand out what they don't have (owner and admins can).
    let mine = state.permissions(guild);
    let grantable = |p: Permissions| mine.contains(Permissions::ADMINISTRATOR) || mine.contains(p);
    rsx! {
        for (section, perms) in ROLE_SECTIONS.iter() {
            div { key: "{section}", class: "perm-section",
                div { class: "field-label", {t(section)} }
                for p in perms.iter().copied() {
                    {
                        let (name, hint) = perm_text(p);
                        let on = value.contains(p);
                        let allowed = editable && grantable(p);
                        rsx! {
                            div { key: "{p.bits()}", class: if allowed { "setting-row" } else { "setting-row disabled" },
                                div { class: "setting-text",
                                    div { class: "setting-title", "{name}" }
                                    div { class: "hint", "{hint}" }
                                }
                                Toggle {
                                    on,
                                    onchange: move |v: bool| {
                                        if allowed {
                                            onchange.call(if v { value | p } else { value - p });
                                        }
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

/// Who has a role, with adding and removing.
#[component]
fn RoleMembers(guild: GuildId, role: RoleId, editable: bool) -> Element {
    let state = use_context::<AppState>();
    let members = state.members.read().get(&guild).cloned().unwrap_or_default();
    let (with, without): (Vec<_>, Vec<_>) = members.into_iter().partition(|m| m.roles.contains(&role));
    let set = move |user: UserId, add: bool| {
        let Some(s) = state.session.cloned() else { return };
        spawn_forever(async move {
            state.report(s.api.set_member_role(guild, user, role, add).await.map_err(|e| e.to_string()));
            state.report(state.refresh().await);
        });
    };
    let mut options = vec![opt("", t("roles.add_member"))];
    options.extend(without.iter().map(|m| opt(m.user.id.to_string(), state.display_name_in(Some(guild), m.user.id))));

    rsx! {
        if editable && !without.is_empty() {
            div { class: "field",
                Select {
                    value: String::new(),
                    options,
                    onchange: move |v: String| {
                        if let Ok(user) = v.parse() {
                            set(user, true);
                        }
                    },
                }
            }
        }
        if with.is_empty() {
            div { class: "muted", {t("roles.no_members")} }
        }
        div { class: "user-rows",
            for m in with {
                {
                    let user = m.user.id;
                    let name = state.display_name_in(Some(guild), user);
                    rsx! {
                        div { key: "{user}", class: "user-row",
                            Avatar { name: name.clone(), id: Some(user), size: 32 }
                            span { class: "user-row-name", "{name}" }
                            span { class: "muted small", "@{m.user.username}" }
                            span { class: "topbar-spacer" }
                            if editable {
                                div { class: "icon-button", title: t("roles.remove_member"), onclick: move |_| set(user, false),
                                    Svg { icon: Icon::X, size: 16 }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_hsv_round_trip() {
        assert_eq!(parse_hex("#3366ff"), Some(0x3366ff));
        assert_eq!(parse_hex("36f"), Some(0x3366ff));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(hex(0x0a0b0c), "#0a0b0c");
        for c in [0xff0000, 0x00ff00, 0x3366ff, 0x99aab5, 0x000000, 0xffffff] {
            let (h, s, v) = to_hsv(c);
            assert_eq!(from_hsv(h, s, v), c, "{c:06x}");
        }
    }
}
