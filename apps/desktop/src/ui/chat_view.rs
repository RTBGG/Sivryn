use chrono::{DateTime, Local, TimeZone};
use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::ChannelId;

use super::icons::{Icon, Svg};
use super::widgets::Avatar;
use super::{AppState, merge_lines};
use crate::chat::{self, ChatLine};
use crate::i18n::t;

/// Messages from the same author within this window share one header.
const GROUP_WINDOW_MS: i64 = 5 * 60 * 1000;

#[component]
pub fn ChatView(channel: ChannelId) -> Element {
    let mut state = use_context::<AppState>();
    let mut draft = use_signal(String::new);

    // Show the stored history right away, then join the channel's MLS group
    // and fetch anything newer.
    use_effect(use_reactive!(|channel| {
        let Some(s) = state.session.cloned() else { return };
        let Some(guild) = state.guild_of(channel) else { return };
        state.unread.write().remove(&channel);
        if !state.messages.peek().contains_key(&channel) {
            let stored = chat::history(&s, channel);
            state.messages.write().insert(channel, stored);
        }
        spawn(async move {
            match chat::ensure_group(&s, guild, channel).await {
                Ok(true) => {
                    state.groups_ready.write().insert(channel);
                }
                Ok(false) => return,
                Err(e) => return state.report(Err(e)),
            }
            match chat::catch_up(&s, channel).await {
                Ok(new) => {
                    if let Some(list) = state.messages.write().get_mut(&channel) {
                        merge_lines(list, new);
                    }
                }
                Err(e) => state.report(Err(e)),
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
                Ok(line) => merge_lines(state.messages.write().entry(channel).or_default(), [line]),
                Err(e) => state.report(Err(e)),
            }
        });
    };

    let name = state.channel_name(channel);
    let messages = state.messages.read();
    let lines = messages.get(&channel).map(Vec::as_slice).unwrap_or_default();
    let compact = state.settings.read().appearance.compact;
    let placeholder = format!("{} #{name}", t("chat.message_to"));

    rsx! {
        div { class: "chat",
            header { class: "topbar",
                span { class: "topbar-icon", Svg { icon: Icon::Hash, size: 20 } }
                span { class: "topbar-title", "{name}" }
                span { class: "e2ee-badge", title: t("chat.e2ee_hint"),
                    Svg { icon: Icon::ShieldCheck, size: 14 }
                    {t("chat.e2ee")}
                }
            }
            div { class: "messages",
                if lines.is_empty() {
                    div { class: "channel-intro",
                        div { class: "intro-icon", Svg { icon: Icon::Hash, size: 32 } }
                        h2 { "{t(\"chat.welcome\")} #{name}" }
                        p { class: "muted", {t("chat.welcome_hint")} }
                    }
                }
                for (i, line) in lines.iter().enumerate() {
                    Fragment { key: "{line.id.map(|id| id.to_string()).unwrap_or_else(|| i.to_string())}",
                        if starts_day(lines, i) {
                            div { class: "day-divider",
                                div { class: "day-line" }
                                span { "{day_label(line.created_at)}" }
                                div { class: "day-line" }
                            }
                        }
                        MessageLine {
                            line: line.clone(),
                            grouped: !compact && continues_group(lines, i),
                            compact,
                        }
                    }
                }
            }
            if ready {
                div { class: "composer-wrap",
                    div { class: "composer",
                        input {
                            value: "{draft}",
                            placeholder: "{placeholder}",
                            oninput: move |e| draft.set(e.value()),
                            onkeydown: move |e| {
                                if e.key() == Key::Enter {
                                    send();
                                }
                            },
                        }
                        div { class: "send-button", onclick: move |_| send(), Svg { icon: Icon::Send, size: 18 } }
                    }
                }
            } else {
                div { class: "composer-wrap",
                    div { class: "composer waiting",
                        Svg { icon: Icon::Lock, size: 16 }
                        span { {t("chat.waiting_for_key")} }
                    }
                }
            }
        }
    }
}

#[component]
fn MessageLine(line: ChatLine, grouped: bool, compact: bool) -> Element {
    let state = use_context::<AppState>();
    let author = state.username(line.author);
    let time = time_label(line.created_at);
    let body = match &line.body {
        Some(body) => rsx! { div { class: "msg-body", "{body}" } },
        None => rsx! { div { class: "msg-body locked", {t("chat.locked")} } },
    };

    if compact {
        return rsx! {
            div { class: "msg compact",
                span { class: "msg-time", "{time}" }
                span { class: "msg-author", "{author}" }
                {body}
            }
        };
    }
    if grouped {
        return rsx! {
            div { class: "msg grouped",
                span { class: "msg-gutter", "{time}" }
                {body}
            }
        };
    }
    rsx! {
        div { class: "msg",
            Avatar { name: author.clone(), id: Some(line.author), size: 38 }
            div { class: "msg-content",
                div { class: "msg-head",
                    span { class: "msg-author", "{author}" }
                    span { class: "msg-time", "{time}" }
                }
                {body}
            }
        }
    }
}

fn local(ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}

fn starts_day(lines: &[ChatLine], i: usize) -> bool {
    let day = |l: &ChatLine| local(l.created_at).map(|d| d.date_naive());
    i == 0 || day(&lines[i]) != day(&lines[i - 1])
}

fn continues_group(lines: &[ChatLine], i: usize) -> bool {
    i > 0
        && !starts_day(lines, i)
        && lines[i].author == lines[i - 1].author
        && lines[i].created_at - lines[i - 1].created_at < GROUP_WINDOW_MS
}

fn time_label(ms: i64) -> String {
    local(ms).map(|d| d.format("%H:%M").to_string()).unwrap_or_default()
}

fn day_label(ms: i64) -> String {
    let Some(d) = local(ms) else { return String::new() };
    let today = Local::now().date_naive();
    match (today - d.date_naive()).num_days() {
        0 => t("chat.today").into(),
        1 => t("chat.yesterday").into(),
        _ => d.format("%d.%m.%Y").to_string(),
    }
}
