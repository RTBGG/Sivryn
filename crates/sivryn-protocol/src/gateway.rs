//! Realtime gateway messages (WebSocket at `/gateway`, JSON text frames).
//!
//! Flow: client connects, sends [`ClientMessage::Identify`], receives
//! [`ServerMessage::Ready`] and from then on a stream of [`Event`]s for the
//! guilds it is a member of. Clients send `Heartbeat` every
//! `heartbeat_interval_ms`; the server closes idle connections.

use serde::{Deserialize, Serialize};

use crate::api::MlsWelcome;
use crate::model::{Channel, Guild, Message, User};
use crate::{ChannelId, DeviceId, GuildId, MessageId, UserId};

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

/// Events pushed to clients. The server decides who receives each one (see
/// [`Audience`]); events carry what clients need to update their state.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Event {
    GuildCreate {
        guild: Guild,
    },
    GuildUpdate {
        guild: Guild,
    },
    GuildDelete {
        guild_id: GuildId,
    },
    ChannelCreate {
        channel: Channel,
    },
    ChannelDelete {
        guild_id: Option<GuildId>,
        channel_id: ChannelId,
    },
    MemberJoin {
        guild_id: GuildId,
        user: User,
    },
    /// Left, kicked or banned.
    MemberLeave {
        guild_id: GuildId,
        user_id: UserId,
    },
    MessageCreate {
        message: Message,
    },
    MessageDelete {
        channel_id: ChannelId,
        message_id: MessageId,
    },
    /// An MLS commit that every device in the channel's group must apply.
    MlsCommit {
        channel_id: ChannelId,
        epoch: u64,
        commit: String,
    },
    /// A welcome addressed to specific devices so they can join a channel's group.
    MlsWelcome {
        channel_id: ChannelId,
        welcome: MlsWelcome,
    },
    /// A DM or group was created, renamed or changed members. Members that
    /// are already in the channel's MLS group add `added` and remove `removed`.
    DmUpdate {
        channel_id: ChannelId,
        #[serde(default)]
        added: Vec<UserId>,
        #[serde(default)]
        removed: Vec<UserId>,
    },
    /// Someone started a call in a DM or group you're in.
    CallRing {
        channel_id: ChannelId,
        user_id: UserId,
    },
    /// Friends, requests or blocks changed; refetch `GET /friends`.
    RelationshipsChanged,
    PresenceUpdate {
        user_id: UserId,
        online: bool,
    },
}

/// Who receives an event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "to", content = "ids", rename_all = "snake_case")]
pub enum Audience {
    /// Every member of the guild.
    Guild(GuildId),
    /// These users, on all their connections.
    Users(Vec<UserId>),
    /// These devices only (MLS welcomes).
    Devices(Vec<DeviceId>),
}

/// An event with its audience, as it travels through the server's event bus.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub to: Audience,
    pub event: Event,
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
