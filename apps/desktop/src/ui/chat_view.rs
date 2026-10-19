use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::ChannelId;
use sivryn_client_core::protocol::api::MessageQuery;

use super::AppState;
use crate::chat;

#[component]
pub fn ChatView(channel: ChannelId) -> Element {
    let mut state = use_context::<AppState>();
    let mut draft = use_signal(String::new);

    // Join the channel's MLS group and load history when the channel opens.
    use_effect(use_reactive!(|channel| {
        let Some(s) = state.session.cloned() else { return };
        let Some(guild) = state.guild_of(channel) else { return };
        spawn(async move {
            match chat::ensure_group(&s, guild, channel).await {
                Ok(true) => {
                    state.groups_ready.write().insert(channel);
                }
                Ok(false) => {}
                Err(e) => return state.report(Err(e)),
            }
            if state.messages.peek().contains_key(&channel) {
                return;
            }
            match s.api.messages(channel, &MessageQuery::default()).await {
                Ok(mut page) => {
                    page.reverse();
                    let lines = page.iter().filter_map(|m| chat::decrypt(&s, m)).collect();
                    state.messages.write().insert(channel, lines);
                }
                Err(e) => state.report(Err(e.to_string())),
            }
        });
    }));

    let ready = state.groups_ready.read().contains(&channel);
    let mut send = move || {
        let body = draft.read().trim().to_owned();
        let Some(s) = state.session.cloned() else { return };
        if body.is_empty() {
            return;
        }
        draft.set(String::new());
        // Don't lose the message if the user switches channels mid-send.
        spawn_forever(async move {
            match chat::send(&s, channel, body).await {
                Ok(line) => state.messages.write().entry(channel).or_default().push(line),
                Err(e) => state.report(Err(e)),
            }
        });
    };

    let messages = state.messages.read();
    let lines = messages.get(&channel).map(Vec::as_slice).unwrap_or_default();

    rsx! {
        div { class: "chat",
            div { class: "messages",
                for (i, line) in lines.iter().enumerate() {
                    div { key: "{i}", class: "message",
                        span { class: "author", "{state.username(line.author)}" }
                        match &line.body {
                            Some(body) => rsx! { span { class: "body", "{body}" } },
                            None => rsx! { span { class: "body locked", "🔒 encrypted before this device joined" } },
                        }
                    }
                }
            }
            if ready {
                div { class: "composer",
                    input {
                        value: "{draft}",
                        placeholder: "Message (end-to-end encrypted)",
                        oninput: move |e| draft.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                send();
                            }
                        },
                    }
                    button { class: "primary", onclick: move |_| send(), "Send" }
                }
            } else {
                div { class: "composer muted", "Waiting for a member to add this device to the encrypted channel…" }
            }
        }
    }
}
