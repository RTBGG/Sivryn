//! A text channel or DM: header, messages and the message box.

use chrono::{DateTime, Local, TimeZone};
use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::model::{ChannelKind, MessageContent, MessageKind};
use sivryn_client_core::protocol::{ChannelId, MessageId, UserId};

use super::icons::{Icon, Svg};
use super::media::{AttachmentView, EmbedView};
use super::widgets::{Avatar, IconButton};
use super::{AppState, Modal, call, emoji_picker, members, menu};
use crate::chat;
use crate::files::{self, PendingFile};
use crate::i18n::t;
use crate::messages::{ChatMessage, fold};
use crate::preview::{self, Segment};

/// Messages from the same author within this window share one header.
const GROUP_WINDOW_MS: i64 = 5 * 60 * 1000;

#[component]
pub fn ChatView(channel: ChannelId) -> Element {
    let mut state = use_context::<AppState>();

    // Show the stored history right away, then join the channel's MLS group
    // and fetch anything newer.
    use_effect(use_reactive!(|channel| {
        let Some(s) = state.session.cloned() else { return };
        state.unread.write().remove(&channel);
        if !state.messages.peek().contains_key(&channel) {
            let stored = fold(&chat::history(&s, channel));
            state.messages.write().insert(channel, stored);
        }
        spawn(async move {
            match chat::ensure_group(&s, channel).await {
                Ok(true) => {
                    state.groups_ready.write().insert(channel);
                }
                Ok(false) => return,
                Err(e) => return state.report(Err(e)),
            }
            match chat::catch_up(&s, channel).await {
                Ok(new) => state.on_new_records(&s, channel, new),
                Err(e) => state.report(Err(e)),
            }
        });
    }));

    let Some(info) = state.find_channel(channel) else { return rsx! {} };
    let kind = info.kind;
    let name = state.channel_name(channel);
    let ready = state.groups_ready.read().contains(&channel);
    let show_members = state.settings.read().member_list && kind != ChannelKind::Dm;
    let messages = state.messages.read();
    let list = messages.get(&channel).map(Vec::as_slice).unwrap_or_default();
    let compact = state.settings.read().appearance.compact;
    let me = state.me();
    let icon = match kind {
        ChannelKind::Dm => Icon::AtSign,
        ChannelKind::Group => Icon::Users,
        _ => Icon::Hash,
    };
    let dm_partner = (kind == ChannelKind::Dm)
        .then(|| state.dm(channel).and_then(|d| d.members.iter().find(|u| Some(u.id) != me).map(|u| u.id)))
        .flatten();

    rsx! {
        div { class: "chat",
            header { class: "topbar",
                span { class: "topbar-icon", Svg { icon, size: 20 } }
                span {
                    class: "topbar-title",
                    onmousedown: move |e| {
                        if let Some((x, y)) = menu::right_click(&e) {
                            menu::open(state, x, y, menu::channel_items(state, channel));
                        }
                    },
                    "{name}"
                }
                if let Some(partner) = dm_partner {
                    if state.online.read().contains(&partner) {
                        span { class: "status-dot online" }
                    }
                }
                span { class: "e2ee-badge", title: t("chat.e2ee_hint"),
                    Svg { icon: Icon::ShieldCheck, size: 14 }
                    {t("chat.e2ee")}
                }
                span { class: "topbar-spacer" }
                if kind.is_private() {
                    IconButton { icon: Icon::Phone, size: 18, title: t("call.start"), onclick: move |_| call::start(state, channel, false) }
                    IconButton { icon: Icon::Video, size: 18, title: t("call.start_video"), onclick: move |_| call::start(state, channel, true) }
                    IconButton { icon: Icon::UserPlus, size: 18, title: t("dm.add_people"), onclick: move |_| state.modal.set(Some(Modal::AddToDm(channel))) }
                }
                if kind != ChannelKind::Dm {
                    IconButton {
                        icon: Icon::Users,
                        size: 18,
                        class: if show_members { "active" } else { "" },
                        title: if show_members { t("members.hide") } else { t("members.show") },
                        onclick: move |_| state.update_settings(|s| s.member_list = !s.member_list),
                    }
                }
            }
            if kind.is_private() {
                call::DmCall { channel }
            }
            div { class: "chat-body",
                div { class: "chat-column",
                    div { class: "messages", "data-stick-bottom": "{channel}",
                        // Pushes a short history to the bottom, next to the message box.
                        div { class: "messages-spacer" }
                        if list.is_empty() {
                            ChannelIntro { kind, name: name.clone(), partner: dm_partner }
                        }
                        for (i, msg) in list.iter().enumerate() {
                            Fragment { key: "{msg.id}",
                                if starts_day(list, i) {
                                    div { class: "day-divider",
                                        div { class: "day-line" }
                                        span { "{day_label(msg.created_at)}" }
                                        div { class: "day-line" }
                                    }
                                }
                                MessageView {
                                    channel,
                                    msg: msg.clone(),
                                    reply: reply_preview(state, list, msg),
                                    grouped: !compact && continues_group(list, i),
                                    compact,
                                }
                            }
                        }
                    }
                    if ready {
                        ComposerView { channel, name: name.clone(), kind }
                    } else {
                        div { class: "composer-wrap",
                            div { class: "composer waiting",
                                Svg { icon: Icon::Lock, size: 16 }
                                span { {t("chat.waiting_for_key")} }
                            }
                        }
                    }
                }
                if show_members {
                    members::MemberList { channel }
                }
            }
        }
    }
}

#[component]
fn ChannelIntro(kind: ChannelKind, name: String, partner: Option<UserId>) -> Element {
    match kind {
        ChannelKind::Dm => rsx! {
            div { class: "channel-intro",
                Avatar { name: name.clone(), id: partner, size: 72 }
                h2 { "{name}" }
                p { class: "muted", "{t(\"chat.dm_start\")} {name}." }
            }
        },
        ChannelKind::Group => rsx! {
            div { class: "channel-intro",
                div { class: "intro-icon", Svg { icon: Icon::Users, size: 32 } }
                h2 { "{name}" }
                p { class: "muted", {t("chat.group_start")} }
            }
        },
        _ => rsx! {
            div { class: "channel-intro",
                div { class: "intro-icon", Svg { icon: Icon::Hash, size: 32 } }
                h2 { "{t(\"chat.welcome\")} #{name}" }
                p { class: "muted", {t("chat.welcome_hint")} }
            }
        },
    }
}

/// Author and a short excerpt of the message this one replies to.
fn reply_preview(state: AppState, list: &[ChatMessage], msg: &ChatMessage) -> Option<(Option<UserId>, String)> {
    let target = msg.content.as_ref()?.reply_to?;
    match list.iter().find(|m| m.id == target) {
        Some(m) => Some((Some(m.author), excerpt(m))),
        // Older than what is loaded: look in the local store.
        None => {
            let stored = state.session.peek().as_ref().and_then(|s| s.store.record(target).ok().flatten());
            match stored.map(|r| fold(&[r])).and_then(|mut m| m.pop()) {
                Some(m) => Some((Some(m.author), excerpt(&m))),
                None => Some((None, t("chat.reply_missing").into())),
            }
        }
    }
}

fn excerpt(m: &ChatMessage) -> String {
    let Some(content) = &m.content else { return t("chat.locked").into() };
    let body: String = content.body.chars().take(90).collect();
    if !body.trim().is_empty() {
        body.replace('\n', " ")
    } else if let Some(a) = content.attachments.first() {
        a.name.clone()
    } else {
        String::new()
    }
}

#[component]
fn MessageView(
    channel: ChannelId,
    msg: ChatMessage,
    reply: Option<(Option<UserId>, String)>,
    grouped: bool,
    compact: bool,
) -> Element {
    let state = use_context::<AppState>();
    let author = state.username(msg.author);
    let time = time_label(msg.created_at);
    let me = state.me();
    let mine = Some(msg.author) == me;
    let editing = state.composer.read().get(&channel).and_then(|c| c.editing) == Some(msg.id);
    let replying = state.composer.read().get(&channel).and_then(|c| c.reply_to) == Some(msg.id);
    let id = msg.id;
    let author_id = msg.author;

    let link = msg.body().and_then(preview::first_link).map(str::to_owned);
    let context = {
        let msg = msg.clone();
        move |e: MouseEvent| {
            if let Some((x, y)) = menu::right_click(&e) {
                e.stop_propagation();
                menu::open(state, x, y, menu::message_items(state, channel, &msg, link.clone()));
            }
        }
    };
    let user_menu = move |e: MouseEvent| {
        if let Some((x, y)) = menu::right_click(&e) {
            e.stop_propagation();
            menu::open(state, x, y, menu::user_items(state, author_id, state.guild_of(channel), None));
        }
    };
    let open_profile = move |_| {
        let mut state = state;
        state.modal.set(Some(Modal::Profile(author_id)));
    };

    let class = match (compact, grouped) {
        (true, _) => "msg compact",
        (false, true) => "msg grouped",
        (false, false) => "msg",
    };
    let highlight = if editing || replying { " highlight" } else { "" };

    let body = rsx! { MessageBody { channel, msg: msg.clone() } };
    let actions = rsx! {
        div { class: "msg-actions",
            div { class: "msg-action", title: t("menu.add_reaction"), onclick: move |_| emoji_picker::open_for_reaction(state, channel, id),
                Svg { icon: Icon::Smile, size: 18 }
            }
            if !mine {
                div { class: "msg-action", title: t("menu.reply"), onclick: move |_| reply_to(state, channel, id),
                    Svg { icon: Icon::Reply, size: 18 }
                }
            }
            if mine && msg.content.is_some() {
                div { class: "msg-action", title: t("menu.edit"), onclick: move |_| start_edit(state, channel, id),
                    Svg { icon: Icon::Pencil, size: 18 }
                }
            }
        }
    };
    let reply_line = reply.map(|(user, text)| {
        let name = user.map(|u| state.username(u)).unwrap_or_default();
        rsx! {
            div { class: "msg-reply",
                div { class: "reply-spine" }
                Svg { icon: Icon::Reply, size: 13 }
                if !name.is_empty() {
                    span { class: "reply-author", "{name}" }
                }
                span { class: "reply-text", "{text}" }
            }
        }
    });

    if compact {
        return rsx! {
            div { class: "msg-wrap",
                {reply_line}
                div { class: "{class}{highlight}", onmousedown: context,
                    span { class: "msg-time", "{time}" }
                    span { class: "msg-author", onclick: open_profile, onmousedown: user_menu, "{author}" }
                    div { class: "msg-content", {body} }
                    {actions}
                }
            }
        };
    }
    if grouped && reply_line.is_none() {
        return rsx! {
            div { class: "{class}{highlight}", onmousedown: context,
                span { class: "msg-gutter", "{time}" }
                div { class: "msg-content", {body} }
                {actions}
            }
        };
    }
    rsx! {
        div { class: "msg{highlight}", onmousedown: context,
            div { class: "msg-avatar", onclick: open_profile, onmousedown: user_menu,
                Avatar { name: author.clone(), id: Some(msg.author), size: 38 }
            }
            div { class: "msg-content",
                {reply_line}
                div { class: "msg-head",
                    span { class: "msg-author", onclick: open_profile, onmousedown: user_menu, "{author}" }
                    span { class: "msg-time", "{time}" }
                }
                {body}
            }
            {actions}
        }
    }
}

/// Text with clickable links, attachments, previews and reactions.
#[component]
fn MessageBody(channel: ChannelId, msg: ChatMessage) -> Element {
    let state = use_context::<AppState>();
    let Some(content) = msg.content.clone() else {
        return rsx! { div { class: "msg-body locked", {t("chat.locked")} } };
    };
    // A link that is only an image shows just the image.
    let hide_text =
        content.embeds.len() == 1 && content.embeds[0].image_only && content.body.trim() == content.embeds[0].url;
    let jumbo = is_emoji_only(&content.body);
    let me = state.me();

    rsx! {
        if !hide_text && !content.body.is_empty() {
            div { class: if jumbo { "msg-body selectable jumbo" } else { "msg-body selectable" },
                for (i, segment) in preview::segments(&content.body).into_iter().enumerate() {
                    match segment {
                        Segment::Text(text) => rsx! { span { key: "{i}", "{text}" } },
                        Segment::Link(url) => {
                            let url = url.to_owned();
                            let open = url.clone();
                            rsx! {
                                span { key: "{i}", class: "link", title: "{url}", onclick: move |_| files::open_link(&open), "{url}" }
                            }
                        }
                    }
                }
                if msg.edited {
                    span { class: "msg-edited", " ({t(\"chat.edited\")})" }
                }
            }
        } else if msg.edited {
            span { class: "msg-edited", "({t(\"chat.edited\")})" }
        }
        if !content.attachments.is_empty() {
            div { class: "attachments",
                for a in content.attachments.iter() {
                    AttachmentView { key: "{a.file_id}", attachment: a.clone() }
                }
            }
        }
        for (i, embed) in content.embeds.iter().enumerate() {
            EmbedView { key: "{i}", embed: embed.clone() }
        }
        if !msg.reactions.is_empty() {
            div { class: "reactions",
                for (emoji, users) in msg.reactions.iter().filter(|(_, u)| !u.is_empty()) {
                    {
                        let mine = me.is_some_and(|me| users.contains(&me));
                        let names: Vec<String> = users.iter().map(|u| state.username(*u)).collect();
                        let emoji = emoji.clone();
                        let e2 = emoji.clone();
                        let id = msg.id;
                        rsx! {
                            div {
                                key: "{emoji}",
                                class: if mine { "reaction mine" } else { "reaction" },
                                title: "{names.join(\", \")}",
                                onclick: move |_| toggle_reaction(state, channel, id, e2.clone()),
                                span { class: "emoji", "{emoji}" }
                                span { class: "reaction-count", "{users.len()}" }
                            }
                        }
                    }
                }
                div {
                    class: "reaction add",
                    title: t("menu.add_reaction"),
                    onclick: move |_| emoji_picker::open_for_reaction(state, channel, msg.id),
                    Svg { icon: Icon::Smile, size: 15 }
                }
            }
        }
    }
}

/// Only emoji (and spaces): shown larger, like elsewhere.
fn is_emoji_only(body: &str) -> bool {
    let trimmed = body.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= 24
        && trimmed.chars().all(|c| c.is_whitespace() || (!c.is_ascii() && !c.is_alphanumeric()))
}

fn reply_to(mut state: AppState, channel: ChannelId, message: MessageId) {
    let mut c = state.composer.write();
    let composer = c.entry(channel).or_default();
    composer.editing = None;
    composer.reply_to = Some(message);
}

/// Puts a message into the box for editing.
pub fn start_edit(mut state: AppState, channel: ChannelId, message: MessageId) {
    let body = state
        .messages
        .peek()
        .get(&channel)
        .and_then(|l| l.iter().find(|m| m.id == message))
        .and_then(|m| m.body().map(str::to_owned));
    let Some(body) = body else { return };
    let mut c = state.composer.write();
    let composer = c.entry(channel).or_default();
    composer.reply_to = None;
    composer.editing = Some(message);
    composer.draft = body;
}

/// Adds my reaction, or removes it if I already reacted with this emoji.
pub fn toggle_reaction(state: AppState, channel: ChannelId, message: MessageId, emoji: String) {
    let Some(s) = state.session.cloned() else { return };
    let me = s.user.id;
    let remove = state
        .messages
        .peek()
        .get(&channel)
        .and_then(|l| l.iter().find(|m| m.id == message))
        .is_some_and(|m| m.reactions.iter().any(|(e, users)| *e == emoji && users.contains(&me)));
    spawn_forever(async move {
        match chat::react(&s, channel, message, &emoji, remove).await {
            Ok(record) => state.on_new_records(&s, channel, vec![record]),
            Err(e) => state.report(Err(e)),
        }
    });
}

/// Deletes after the user confirmed.
pub fn delete(mut state: AppState, channel: ChannelId, message: MessageId) {
    let Some(s) = state.session.cloned() else { return };
    state.modal.set(None);
    spawn_forever(async move {
        match chat::delete(&s, channel, message).await {
            Ok(()) => {
                if let Some(list) = state.messages.write().get_mut(&channel) {
                    list.retain(|m| m.id != message);
                }
            }
            Err(e) => state.report(Err(e)),
        }
    });
}

fn add_files(mut state: AppState, channel: ChannelId, files: Vec<PendingFile>) {
    if files.is_empty() {
        return;
    }
    state.composer.write().entry(channel).or_default().files.extend(files);
}

/// Reads files off the UI thread and adds them to the box.
fn add_paths(state: AppState, channel: ChannelId, paths: Vec<std::path::PathBuf>) {
    spawn_forever(async move {
        let read =
            tokio::task::spawn_blocking(move || paths.iter().map(|p| files::prepare_path(p)).collect::<Vec<_>>()).await;
        let Ok(results) = read else { return };
        let mut ok = Vec::new();
        for r in results {
            match r {
                Ok(f) => ok.push(f),
                Err(e) => state.report(Err(e)),
            }
        }
        add_files(state, channel, ok);
    });
}

/// Files dragged onto the window go to the open channel's message box.
pub fn handle_file_drop(mut state: AppState, event: blitz_shell::file_drop::FileDrop) {
    use blitz_shell::file_drop::FileDrop;
    let target = (*state.channel.peek())
        .filter(|c| state.groups_ready.peek().contains(c))
        .filter(|c| state.find_channel(*c).is_some_and(|ch| ch.kind.has_messages()));
    match event {
        FileDrop::Hovered(_) => {
            if target.is_some() && !*state.drag_over.peek() {
                state.drag_over.set(true);
            }
        }
        FileDrop::Cancelled => state.drag_over.set(false),
        FileDrop::Dropped(path) => {
            state.drag_over.set(false);
            match target {
                Some(channel) => add_paths(state, channel, vec![path]),
                None => state.toast(t("chat.drop_nowhere"), false),
            }
        }
    }
}

fn pick_files(state: AppState, channel: ChannelId) {
    spawn_forever(async move {
        let Some(handles) = rfd::AsyncFileDialog::new().set_title(t("chat.attach")).pick_files().await else { return };
        let paths = handles.iter().map(|h| h.path().to_path_buf()).collect();
        add_paths(state, channel, paths);
    });
}

fn send(mut state: AppState, channel: ChannelId) {
    let Some(s) = state.session.cloned() else { return };
    let c = state.composer.peek().get(&channel).cloned().unwrap_or_default();
    if c.sending {
        return;
    }
    let body = c.draft.trim().to_owned();
    let previews = state.settings.peek().link_previews;

    if let Some(target) = c.editing {
        let original = state.messages.peek().get(&channel).and_then(|l| l.iter().find(|m| m.id == target).cloned());
        {
            let mut all = state.composer.write();
            let composer = all.entry(channel).or_default();
            composer.editing = None;
            composer.draft.clear();
        }
        let Some(original) = original.and_then(|m| m.content) else { return };
        // Emptying a message without attachments means deleting it.
        if body.is_empty() && original.attachments.is_empty() {
            state.modal.set(Some(Modal::ConfirmDelete(channel, target)));
            return;
        }
        if body == original.body {
            return;
        }
        spawn_forever(async move {
            let mut content = MessageContent { body: body.clone(), ..Default::default() };
            if previews && let Some(url) = preview::first_link(&body) {
                content.embeds = original.embeds.iter().filter(|e| e.url == url).cloned().collect();
                if content.embeds.is_empty() {
                    content.embeds = preview::build(&s, channel, url).await.into_iter().collect();
                }
            }
            match chat::send(&s, channel, content, MessageKind::Edit, Some(target)).await {
                Ok(record) => state.on_new_records(&s, channel, vec![record]),
                Err(e) => state.report(Err(e)),
            }
        });
        return;
    }

    if body.is_empty() && c.files.is_empty() {
        return;
    }
    {
        let mut all = state.composer.write();
        let composer = all.entry(channel).or_default();
        composer.draft.clear();
        composer.reply_to = None;
        composer.sending = !c.files.is_empty();
    }
    // Keeps running when the user switches channels mid-send.
    spawn_forever(async move {
        let sent_files = c.files.clone();
        let mut finish = move |sent: bool| {
            let mut all = state.composer.write();
            let composer = all.entry(channel).or_default();
            composer.sending = false;
            if sent {
                composer.files.retain(|f| !sent_files.iter().any(|g| std::sync::Arc::ptr_eq(&f.bytes, &g.bytes)));
            }
        };
        let mut attachments = Vec::new();
        for file in &c.files {
            match files::upload(&s, channel, file).await {
                Ok(a) => attachments.push(a),
                Err(e) => {
                    finish(false);
                    return state.report(Err(format!("{}: {e}", file.name)));
                }
            }
        }
        let mut embeds = Vec::new();
        if previews && let Some(url) = preview::first_link(&body) {
            embeds.extend(preview::build(&s, channel, url).await);
        }
        let content = MessageContent { body, reply_to: c.reply_to, attachments, embeds, reaction: None };
        let result = chat::send(&s, channel, content, MessageKind::Message, None).await;
        finish(result.is_ok());
        match result {
            Ok(record) => state.on_new_records(&s, channel, vec![record]),
            Err(e) => state.report(Err(e)),
        }
    });
}

/// The last message I can edit, for the Up key.
fn last_own_message(state: AppState, channel: ChannelId) -> Option<MessageId> {
    let me = state.me()?;
    state.messages.peek().get(&channel)?.iter().rev().find(|m| m.author == me && m.content.is_some()).map(|m| m.id)
}

#[component]
fn ComposerView(channel: ChannelId, name: String, kind: ChannelKind) -> Element {
    let mut state = use_context::<AppState>();
    let composer = state.composer.read().get(&channel).cloned().unwrap_or_default();
    let placeholder = match kind {
        ChannelKind::Dm | ChannelKind::Group => format!("{} {name}", t("chat.message_to")),
        _ => format!("{} #{name}", t("chat.message_to")),
    };
    let reply_name = composer.reply_to.and_then(|id| {
        state
            .messages
            .read()
            .get(&channel)
            .and_then(|l| l.iter().find(|m| m.id == id))
            .map(|m| state.username(m.author))
    });
    let drag = *state.drag_over.read();
    let mut cancel = move || {
        let mut all = state.composer.write();
        let c = all.entry(channel).or_default();
        if c.editing.take().is_some() {
            c.draft.clear();
        }
        c.reply_to = None;
    };

    rsx! {
        div { class: "composer-wrap",
            if let Some(name) = reply_name {
                div { class: "composer-bar",
                    span { "{t(\"chat.replying_to\")} " strong { "{name}" } }
                    div { class: "bar-close", onclick: move |_| cancel(), Svg { icon: Icon::X, size: 14 } }
                }
            }
            if composer.editing.is_some() {
                div { class: "composer-bar",
                    span { {t("chat.editing")} }
                    span { class: "muted small", {t("chat.editing_hint")} }
                    div { class: "bar-close", onclick: move |_| cancel(), Svg { icon: Icon::X, size: 14 } }
                }
            }
            if !composer.files.is_empty() {
                div { class: if composer.sending { "pending-files sending" } else { "pending-files" },
                    for (i, f) in composer.files.iter().enumerate() {
                        div { key: "{i}", class: "pending-file",
                            div { class: "pending-icon",
                                if f.is_image() { Svg { icon: Icon::Image, size: 26 } } else { Svg { icon: Icon::File, size: 26 } }
                            }
                            div { class: "pending-name", "{f.name}" }
                            div { class: "muted small", "{files::human_size(f.bytes.len() as u64)}" }
                            if !composer.sending {
                                div {
                                    class: "pending-remove",
                                    title: t("common.remove"),
                                    onclick: move |_| {
                                        if let Some(c) = state.composer.write().get_mut(&channel)
                                            && i < c.files.len()
                                        {
                                            c.files.remove(i);
                                        }
                                    },
                                    Svg { icon: Icon::Trash, size: 15 }
                                }
                            }
                        }
                    }
                    if composer.sending {
                        div { class: "pending-status", div { class: "spinner small" } span { {t("chat.uploading")} } }
                    }
                }
            }
            div { class: if drag { "composer drop" } else { "composer" },
                div { class: "composer-button", title: t("chat.attach"), onclick: move |_| pick_files(state, channel),
                    Svg { icon: Icon::CirclePlus, size: 20 }
                }
                if drag {
                    span { class: "drop-hint", Svg { icon: Icon::Upload, size: 16 } {t("chat.drop_here")} }
                } else {
                    input {
                        value: "{composer.draft}",
                        placeholder: "{placeholder}",
                        oninput: move |e| state.composer.write().entry(channel).or_default().draft = e.value(),
                        onkeydown: move |e| match e.key() {
                            Key::Enter => send(state, channel),
                            Key::Escape => cancel(),
                            Key::ArrowUp => {
                                let empty = state.composer.peek().get(&channel).is_none_or(|c| c.draft.is_empty());
                                if empty && let Some(id) = last_own_message(state, channel) {
                                    start_edit(state, channel, id);
                                }
                            }
                            _ => {}
                        },
                    }
                }
                div { class: "composer-button", title: t("emoji.title"), onclick: move |_| emoji_picker::open_for_composer(state, channel),
                    Svg { icon: Icon::Smile, size: 20 }
                }
                div { class: "send-button", onclick: move |_| send(state, channel), Svg { icon: Icon::Send, size: 18 } }
            }
        }
    }
}

/// "Delete message?" dialog.
#[component]
pub fn ConfirmDelete(channel: ChannelId, message: MessageId) -> Element {
    let mut state = use_context::<AppState>();
    let preview = state
        .messages
        .read()
        .get(&channel)
        .and_then(|l| l.iter().find(|m| m.id == message).map(excerpt))
        .unwrap_or_default();
    rsx! {
        div { class: "overlay", onclick: move |_| state.modal.set(None),
            div { class: "dialog", onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { {t("chat.delete_title")} }
                    IconButton { icon: Icon::X, size: 18, onclick: move |_| state.modal.set(None) }
                }
                p { class: "muted", {t("chat.delete_hint")} }
                if !preview.is_empty() {
                    div { class: "delete-preview", "{preview}" }
                }
                div { class: "dialog-actions",
                    button { class: "button ghost", onclick: move |_| state.modal.set(None), {t("common.cancel")} }
                    button { class: "button danger", onclick: move |_| delete(state, channel, message), {t("menu.delete")} }
                }
            }
        }
    }
}

fn local(ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}

fn starts_day(list: &[ChatMessage], i: usize) -> bool {
    let day = |m: &ChatMessage| local(m.created_at).map(|d| d.date_naive());
    i == 0 || day(&list[i]) != day(&list[i - 1])
}

fn continues_group(list: &[ChatMessage], i: usize) -> bool {
    i > 0
        && !starts_day(list, i)
        && list[i].author == list[i - 1].author
        && list[i].created_at - list[i - 1].created_at < GROUP_WINDOW_MS
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

#[cfg(test)]
mod tests {
    use super::is_emoji_only;

    #[test]
    fn emoji_only_messages() {
        assert!(is_emoji_only("👍"));
        assert!(is_emoji_only(" 🎉 🔥 "));
        assert!(!is_emoji_only("ok 👍"));
        assert!(!is_emoji_only("äöü"));
        assert!(!is_emoji_only(""));
    }
}
