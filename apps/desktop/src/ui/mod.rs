//! Application state and the root component.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use dioxus_native::prelude::*;
use sivryn_client_core::connect_gateway;
use sivryn_client_core::protocol::api::{GuildWithChannels, VoiceJoinResponse};
use sivryn_client_core::protocol::gateway::{Event, ServerMessage};
use sivryn_client_core::protocol::model::ChannelKind;
use sivryn_client_core::protocol::{ChannelId, GuildId, UserId};
use sivryn_media::{ScreenShare, VideoFeed, VoiceCall};

use crate::chat::{self, ChatLine};
use crate::session::Session;

mod call;
mod chat_view;
mod login;
mod shell;

const STYLE: &str = include_str!("style.css");

#[derive(Clone)]
pub struct CallState {
    pub channel: ChannelId,
    pub join: VoiceJoinResponse,
    pub call: Rc<RefCell<Option<VoiceCall>>>,
    pub share: Rc<RefCell<Option<ScreenShare>>>,
    pub feeds: Vec<VideoFeed>,
    pub speakers: Vec<String>,
    pub muted: bool,
    pub sharing: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Modal {
    CreateGuild,
    JoinGuild,
    PickScreen,
}

/// Global UI state, provided as context by [`App`].
#[derive(Clone, Copy)]
pub struct AppState {
    pub session: Signal<Option<Session>>,
    pub guilds: Signal<Vec<GuildWithChannels>>,
    pub users: Signal<HashMap<UserId, String>>,
    pub guild: Signal<Option<GuildId>>,
    pub channel: Signal<Option<ChannelId>>,
    pub messages: Signal<HashMap<ChannelId, Vec<ChatLine>>>,
    /// Channels whose MLS group this device is in.
    pub groups_ready: Signal<HashSet<ChannelId>>,
    pub call: Signal<Option<CallState>>,
    pub modal: Signal<Option<Modal>>,
    pub status: Signal<Option<String>>,
}

impl AppState {
    pub fn report(mut self, result: Result<(), String>) {
        if let Err(e) = result {
            tracing::warn!(error = %e);
            self.status.set(Some(e));
        }
    }

    pub fn username(&self, id: UserId) -> String {
        self.users.read().get(&id).cloned().unwrap_or_else(|| "unknown".into())
    }

    pub fn channel_kind(&self, id: ChannelId) -> Option<ChannelKind> {
        self.guilds.read().iter().flat_map(|g| &g.channels).find(|c| c.id == id).map(|c| c.kind)
    }

    pub fn guild_of(&self, id: ChannelId) -> Option<GuildId> {
        self.guilds.read().iter().flat_map(|g| &g.channels).find(|c| c.id == id).map(|c| c.guild_id)
    }

    /// Reloads guilds and member names after membership changes.
    pub async fn refresh(mut self) -> Result<(), String> {
        let Some(s) = self.session.cloned() else { return Ok(()) };
        let guilds = s.api.guilds().await.map_err(|e| e.to_string())?;
        let mut users = HashMap::from([(s.user.id, s.user.username.clone())]);
        for g in &guilds {
            for m in s.api.members(g.guild.id).await.map_err(|e| e.to_string())? {
                users.insert(m.id, m.username);
            }
        }
        if self.guild.peek().is_none() {
            self.guild.set(guilds.first().map(|g| g.guild.id));
        }
        self.guilds.set(guilds);
        self.users.set(users);
        Ok(())
    }

    /// Called once after login: loads state and pumps gateway events forever.
    pub async fn run_gateway(self) {
        self.report(self.refresh().await);
        let Some(s) = self.session.cloned() else { return };
        let mut conn = match connect_gateway(&s.api, Some(s.device_id)).await {
            Ok(c) => c,
            Err(e) => return self.report(Err(format!("gateway: {e}"))),
        };
        while let Some(msg) = conn.events.recv().await {
            match msg {
                ServerMessage::Event(event) => self.handle_event(&s, event).await,
                ServerMessage::InvalidSession { reason } => {
                    return self.report(Err(format!("session ended: {reason}")));
                }
                _ => {}
            }
        }
        self.report(Err("disconnected from server".into()));
    }

    async fn handle_event(mut self, s: &Session, event: Event) {
        match event {
            Event::MessageCreate { message, .. } => {
                if let Some(line) = chat::decrypt(s, &message) {
                    self.messages.write().entry(message.channel_id).or_default().push(line);
                }
            }
            Event::MlsCommit { channel_id, commit, .. } => {
                if let Some(epoch) = chat::apply_commit(s, channel_id, &commit) {
                    self.on_epoch_change(s, channel_id, epoch);
                }
            }
            Event::MlsWelcome { channel_id, .. } => {
                self.report(chat::accept_welcomes(s).await);
                if s.mls.borrow().has_group(channel_id) {
                    self.groups_ready.write().insert(channel_id);
                }
            }
            Event::MemberJoin { guild_id, user } => {
                if user.id != s.user.id {
                    let channels: Vec<ChannelId> = self
                        .guilds
                        .read()
                        .iter()
                        .filter(|g| g.guild.id == guild_id)
                        .flat_map(|g| g.channels.iter().map(|c| c.id))
                        .collect();
                    for channel in channels {
                        self.report(chat::add_user(s, channel, user.id).await);
                    }
                }
                self.report(self.refresh().await);
            }
            Event::GuildCreate { .. }
            | Event::GuildDelete { .. }
            | Event::ChannelCreate { .. }
            | Event::ChannelDelete { .. }
            | Event::MemberLeave { .. } => self.report(self.refresh().await),
            _ => {}
        }
    }

    /// A new MLS epoch means a new media key for E2EE voice channels.
    fn on_epoch_change(&self, s: &Session, channel: ChannelId, epoch: u64) {
        let call = self.call.read();
        let Some(call) = call.as_ref().filter(|c| c.channel == channel && c.join.e2ee) else { return };
        if let (Ok(key), Some(vc)) = (s.mls.borrow().media_key(channel), call.call.borrow().as_ref()) {
            vc.update_media_key(key, epoch);
        }
    }
}

#[component]
pub fn App() -> Element {
    let state = AppState {
        session: use_signal(|| None),
        guilds: use_signal(Vec::new),
        users: use_signal(HashMap::new),
        guild: use_signal(|| None),
        channel: use_signal(|| None),
        messages: use_signal(HashMap::new),
        groups_ready: use_signal(HashSet::new),
        call: use_signal(|| None),
        modal: use_signal(|| None),
        status: use_signal(|| None),
    };
    use_context_provider(|| state);

    rsx! {
        style { {STYLE} }
        if state.session.read().is_some() {
            shell::Shell {}
        } else {
            login::Login {}
        }
    }
}
