//! Realtime gateway messages (WebSocket at `/gateway`, JSON text frames).
//!
//! Flow: client connects, sends [`ClientMessage::Identify`], receives
//! [`ServerMessage::Ready`] and from then on a stream of [`Event`]s for the
//! guilds it is a member of. Clients send `Heartbeat` every
//! `heartbeat_interval_ms`; the server closes idle connections.

use serde::{Deserialize, Serialize};

use crate::api::MlsWelcome;
use crate::model::{Channel, Guild, Message, User, VoiceParticipant};
use crate::{ChannelId, DeviceId, GuildId, UserId};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", content = "d", rename_all = "snake_case")]
pub enum ClientMessage {
    Identify { token: String, device_id: Option<DeviceId> },
    Heartbeat { seq: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", content = "d", rename_all = "snake_case")]
pub enum ServerMessage {
    Hello { heartbeat_interval_ms: u64, version: u32 },
    Ready { user: User, guilds: Vec<Guild> },
    HeartbeatAck { seq: u64 },
    Event(Event),
    InvalidSession { reason: String },
}

/// Events fanned out through the event bus.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Event {
    GuildCreate {
        guild: Guild,
    },
    GuildDelete {
        guild_id: GuildId,
    },
    ChannelCreate {
        channel: Channel,
    },
    ChannelDelete {
        guild_id: GuildId,
        channel_id: ChannelId,
    },
    MemberJoin {
        guild_id: GuildId,
        user: User,
    },
    MemberLeave {
        guild_id: GuildId,
        user_id: UserId,
    },
    MessageCreate {
        guild_id: GuildId,
        message: Message,
    },
    /// An MLS commit that every device in the channel's group must apply.
    MlsCommit {
        guild_id: GuildId,
        channel_id: ChannelId,
        epoch: u64,
        commit: String,
    },
    /// A welcome addressed to specific devices so they can join a channel's group.
    MlsWelcome {
        channel_id: ChannelId,
        welcome: MlsWelcome,
    },
    VoiceStateUpdate {
        guild_id: GuildId,
        state: VoiceParticipant,
    },
    VoiceLeave {
        guild_id: GuildId,
        channel_id: ChannelId,
        user_id: UserId,
    },
    PresenceUpdate {
        user_id: UserId,
        online: bool,
    },
}

impl Event {
    /// The guild an event belongs to, used to route it to subscribers.
    /// `None` means the event is addressed by other means (e.g. device IDs).
    pub fn guild_id(&self) -> Option<GuildId> {
        match self {
            Event::GuildCreate { guild } => Some(guild.id),
            Event::ChannelCreate { channel } => Some(channel.guild_id),
            Event::GuildDelete { guild_id }
            | Event::ChannelDelete { guild_id, .. }
            | Event::MemberJoin { guild_id, .. }
            | Event::MemberLeave { guild_id, .. }
            | Event::MessageCreate { guild_id, .. }
            | Event::MlsCommit { guild_id, .. }
            | Event::VoiceStateUpdate { guild_id, .. }
            | Event::VoiceLeave { guild_id, .. } => Some(*guild_id),
            Event::MlsWelcome { .. } | Event::PresenceUpdate { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_message_wire_format() {
        let msg = ClientMessage::Heartbeat { seq: 7 };
        assert_eq!(serde_json::to_string(&msg).unwrap(), r#"{"op":"heartbeat","d":{"seq":7}}"#);
    }

    #[test]
    fn event_round_trip() {
        let ev = Event::PresenceUpdate { user_id: UserId::new(), online: true };
        let json = serde_json::to_string(&ServerMessage::Event(ev)).unwrap();
        assert!(json.contains(r#""op":"event""#));
        assert!(json.contains(r#""t":"presence_update""#));
        let back: ServerMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, ServerMessage::Event(Event::PresenceUpdate { online: true, .. })));
    }
}
