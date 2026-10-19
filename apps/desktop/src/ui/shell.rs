//! Main layout: guild rail, channel sidebar and the content area.

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::model::ChannelKind;

use super::{AppState, Modal, call, chat_view};

#[component]
pub fn Shell() -> Element {
    let state = use_context::<AppState>();
    let channel = *state.channel.read();
    let in_call_here = state.call.read().as_ref().is_some_and(|c| Some(c.channel) == channel);

    rsx! {
        div { class: "app",
            GuildRail {}
            Sidebar {}
            div { class: "content",
                if let Some(err) = state.status.read().as_ref() {
                    div { class: "banner", "{err}" }
                }
                match channel.and_then(|c| state.channel_kind(c)) {
                    Some(ChannelKind::Text) => rsx! { chat_view::ChatView { channel: channel.unwrap() } },
                    Some(ChannelKind::Voice) if in_call_here => rsx! { call::CallView {} },
                    Some(ChannelKind::Voice) => rsx! { call::JoinPrompt { channel: channel.unwrap() } },
                    None => rsx! { div { class: "empty", "Pick a channel" } },
                }
            }
            if state.modal.read().is_some() {
                ModalView {}
            }
        }
    }
}

#[component]
fn GuildRail() -> Element {
    let mut state = use_context::<AppState>();
    let selected = *state.guild.read();

    rsx! {
        nav { class: "rail",
            for g in state.guilds.read().iter() {
                div {
                    key: "{g.guild.id}",
                    class: if Some(g.guild.id) == selected { "guild active" } else { "guild" },
                    title: "{g.guild.name}",
                    onclick: {
                        let id = g.guild.id;
                        move |_| {
                            state.guild.set(Some(id));
                            state.channel.set(None);
                        }
                    },
                    "{initials(&g.guild.name)}"
                }
            }
            div { class: "guild add", title: "Create a server", onclick: move |_| state.modal.set(Some(Modal::CreateGuild)), "+" }
            div { class: "guild add", title: "Join with invite", onclick: move |_| state.modal.set(Some(Modal::JoinGuild)), "→" }
        }
    }
}

#[component]
fn Sidebar() -> Element {
    let mut state = use_context::<AppState>();
    let mut invite = use_signal(|| None::<String>);
    let guilds = state.guilds.read();
    let Some(guild) = guilds.iter().find(|g| Some(g.guild.id) == *state.guild.read()) else {
        return rsx! { aside { class: "sidebar" } };
    };
    let guild_id = guild.guild.id;
    let selected = *state.channel.read();
    let me = state.session.read().as_ref().map(|s| s.user.username.clone()).unwrap_or_default();

    rsx! {
        aside { class: "sidebar",
            header { class: "guild-name", "{guild.guild.name}" }
            div { class: "channels",
                for c in guild.channels.iter() {
                    div {
                        key: "{c.id}",
                        class: if Some(c.id) == selected { "channel active" } else { "channel" },
                        onclick: {
                            let id = c.id;
                            move |_| state.channel.set(Some(id))
                        },
                        span { class: "sigil", if c.kind == ChannelKind::Voice { "🔊" } else { "#" } }
                        "{c.name}"
                    }
                }
            }
            div { class: "invite",
                button {
                    onclick: move |_| {
                        let Some(s) = state.session.cloned() else { return };
                        spawn(async move {
                            match s.api.create_invite(guild_id).await {
                                Ok(code) => invite.set(Some(code)),
                                Err(e) => state.report(Err(e.to_string())),
                            }
                        });
                    },
                    "Invite people"
                }
                if let Some(code) = invite.read().as_ref() {
                    p { class: "muted", "Invite code: {code}" }
                }
            }
            footer { class: "me", "{me}" }
        }
    }
}

#[component]
fn ModalView() -> Element {
    let mut state = use_context::<AppState>();
    let mut text = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let Some(modal) = *state.modal.read() else { return rsx! {} };

    if modal == Modal::PickScreen {
        return rsx! { call::ScreenPicker {} };
    }

    let (title, placeholder) = match modal {
        Modal::CreateGuild => ("Create a server", "Server name"),
        _ => ("Join a server", "Invite code"),
    };

    // The dialog stays open until the request is done: a task spawned here is
    // cancelled as soon as this component unmounts.
    let mut confirm = move || {
        let Some(s) = state.session.cloned() else { return };
        let value = text.read().trim().to_owned();
        if value.is_empty() || *busy.read() {
            return;
        }
        busy.set(true);
        error.set(None);
        spawn(async move {
            let result = match modal {
                Modal::CreateGuild => s.api.create_guild(&value).await,
                _ => s.api.accept_invite(&value).await,
            };
            busy.set(false);
            match result {
                Ok(g) => {
                    state.guild.set(Some(g.guild.id));
                    state.channel.set(g.channels.iter().find(|c| c.kind == ChannelKind::Text).map(|c| c.id));
                    state.modal.set(None);
                    // Keep refreshing even though this dialog is gone now.
                    spawn_forever(async move { state.report(state.refresh().await) });
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    rsx! {
        div { class: "overlay",
            div { class: "card",
                h2 { "{title}" }
                input {
                    autofocus: "true",
                    value: "{text}",
                    placeholder: "{placeholder}",
                    oninput: move |e| text.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            confirm();
                        }
                    },
                }
                if let Some(err) = error.read().as_ref() {
                    p { class: "error", "{err}" }
                }
                div { class: "row",
                    button { onclick: move |_| state.modal.set(None), "Cancel" }
                    button { class: "primary", onclick: move |_| confirm(),
                        if *busy.read() { "…" } else { "OK" }
                    }
                }
            }
        }
    }
}

fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase()
}
