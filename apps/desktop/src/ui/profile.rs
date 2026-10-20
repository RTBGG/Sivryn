//! A user's profile.

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::UserId;
use sivryn_client_core::protocol::api::{Relationship, UserProfile};

use super::AppState;
use super::icons::{Icon, Svg};
use super::widgets::{Avatar, IconButton, RemoteImage, StatusDot};
use crate::i18n::t;

#[derive(Clone, Copy)]
enum Action {
    Accept,
    Cancel,
    Remove,
    Block,
    Unblock,
}

#[component]
pub fn ProfileDialog(user: UserId) -> Element {
    let mut state = use_context::<AppState>();
    let mut profile = use_signal(|| None::<UserProfile>);
    // Reload when relationships change (e.g. after "add friend" here).
    let friends_version =
        state.friends.read().friends.len() + state.friends.read().outgoing.len() + state.friends.read().blocked.len();
    use_effect(use_reactive!(|user, friends_version| {
        let _ = friends_version;
        let Some(s) = state.session.cloned() else { return };
        spawn(async move {
            match s.api.profile(user).await {
                Ok(p) => profile.set(Some(p)),
                Err(e) => state.report(Err(e.to_string())),
            }
        });
    }));

    let me = state.me() == Some(user);
    let status = state.status_of(user);
    let close = move |_| state.modal.set(None);
    let Some(p) = profile.read().clone() else {
        return rsx! {
            div { class: "overlay", onclick: close,
                div { class: "dialog profile", onclick: move |e| e.stop_propagation(), div { class: "spinner" } }
            }
        };
    };
    // In a guild, the name chosen there comes first.
    let guild = if *state.home.peek() { None } else { *state.guild.peek() };
    let name = state.display_name_in(guild, user);
    let global = p.user.name().to_owned();
    let bio = p.bio.clone();
    let banner = p.banner_id;
    let since = p.user.created_at.with_timezone(&chrono::Local).format("%d.%m.%Y").to_string();
    let guilds: Vec<String> = p.mutual_guilds.iter().filter_map(|g| state.guild(*g)).map(|g| g.name).collect();
    let api = move |action: Action| {
        let Some(s) = state.session.cloned() else { return };
        spawn_forever(async move {
            let result = match action {
                Action::Accept => s.api.accept_friend(user).await,
                Action::Cancel => s.api.cancel_friend_request(user).await,
                Action::Remove => s.api.remove_friend(user).await,
                Action::Block => s.api.block(user).await,
                Action::Unblock => s.api.unblock(user).await,
            };
            state.report(result.map_err(|e| e.to_string()));
            state.report(state.refresh_friends().await);
        });
    };
    let username = p.user.username.clone();

    rsx! {
        div { class: "overlay", onclick: close,
            div { class: "dialog profile", onclick: move |e| e.stop_propagation(),
                div { class: "profile-banner",
                    for file in banner.iter() {
                        RemoteImage { key: "{file}", file: *file, size: 800 }
                    }
                }
                div { class: "profile-head",
                    div { class: "profile-avatar",
                        Avatar { name: name.clone(), id: Some(user), size: 84 }
                        StatusDot { status, class: "large" }
                    }
                    IconButton { icon: Icon::X, size: 18, onclick: close }
                }
                div { class: "profile-name", "{name}" }
                div { class: "muted",
                    "@{p.user.username}"
                    if global != name {
                        " · {global}"
                    }
                }
                if let Some(s) = status {
                    div { class: "muted small", "{super::shell::status_label(s)}" }
                }
                if !bio.is_empty() {
                    div { class: "profile-section",
                        div { class: "field-label", {t("profile.about")} }
                        div { class: "profile-bio selectable", "{bio}" }
                    }
                }
                div { class: "profile-section",
                    div { class: "field-label", {t("profile.member_since")} }
                    div { "{since}" }
                }
                if !guilds.is_empty() {
                    div { class: "profile-section",
                        div { class: "field-label", {t("profile.mutual_servers")} }
                        div { "{guilds.join(\", \")}" }
                    }
                }
                if me {
                    div { class: "profile-actions",
                        button { class: "button primary", onclick: move |_| {
                                state.modal.set(None);
                                state.settings_page.set(Some(super::SettingsPage::Account));
                            },
                            Svg { icon: Icon::Pencil, size: 16 }
                            {t("profile.edit")}
                        }
                    }
                }
                if !me {
                    div { class: "profile-actions",
                        if p.relationship != Relationship::Blocked {
                            button { class: "button primary", onclick: move |_| {
                                    state.modal.set(None);
                                    state.message_users(vec![user]);
                                },
                                Svg { icon: Icon::Message, size: 16 }
                                {t("menu.message")}
                            }
                        }
                        match p.relationship {
                            Relationship::None => rsx! {
                                button { class: "button", onclick: move |_| {
                                        let name = username.clone();
                                        let Some(s) = state.session.cloned() else { return };
                                        spawn_forever(async move {
                                            match s.api.send_friend_request(&name).await {
                                                Ok(()) => state.toast(t("friends.request_sent"), false),
                                                Err(e) => state.report(Err(e.to_string())),
                                            }
                                            state.report(state.refresh_friends().await);
                                        });
                                    },
                                    Svg { icon: Icon::UserPlus, size: 16 }
                                    {t("menu.add_friend")}
                                }
                            },
                            Relationship::Incoming => rsx! {
                                button { class: "button", onclick: move |_| api(Action::Accept),
                                    Svg { icon: Icon::Check, size: 16 }
                                    {t("friends.accept")}
                                }
                            },
                            Relationship::Outgoing => rsx! {
                                button { class: "button ghost", onclick: move |_| api(Action::Cancel),
                                    {t("friends.cancel_request")}
                                }
                            },
                            Relationship::Friend => rsx! {
                                button { class: "button ghost", onclick: move |_| api(Action::Remove),
                                    Svg { icon: Icon::UserMinus, size: 16 }
                                    {t("menu.remove_friend")}
                                }
                            },
                            Relationship::Blocked => rsx! {
                                button { class: "button", onclick: move |_| api(Action::Unblock),
                                    {t("menu.unblock")}
                                }
                            },
                        }
                        if p.relationship != Relationship::Blocked {
                            button { class: "button danger-ghost", onclick: move |_| api(Action::Block),
                                Svg { icon: Icon::Ban, size: 16 }
                                {t("menu.block")}
                            }
                        }
                    }
                }
            }
        }
    }
}
