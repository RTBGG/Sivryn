//! Domain objects as they appear on the wire.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::permissions::Permissions;
use crate::{ChannelId, DeviceId, GuildId, MessageId, RoleId, UserId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guild {
    pub id: GuildId,
    pub name: String,
    pub owner_id: UserId,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Text,
    Voice,
}

impl ChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Voice => "voice",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "text" => Some(Self::Text),
            "voice" => Some(Self::Voice),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub guild_id: GuildId,
    pub name: String,
    pub kind: ChannelKind,
    pub position: i32,
    /// Voice channels can require end-to-end encrypted media.
    pub e2ee: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    pub id: RoleId,
    pub guild_id: GuildId,
    pub name: String,
    pub permissions: Permissions,
    pub position: i32,
}

/// A chat message.
///
/// Text channels are end-to-end encrypted with MLS: the server only stores and
/// relays `ciphertext` (a serialized MLS `PrivateMessage`) and never sees the
/// plaintext. Clients decrypt it with their MLS group state for the channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub channel_id: ChannelId,
    pub author_id: UserId,
    pub sender_device_id: DeviceId,
    /// MLS epoch the message was encrypted in.
    pub epoch: u64,
    /// Base64 (URL-safe, unpadded) MLS ciphertext.
    pub ciphertext: String,
    pub created_at: DateTime<Utc>,
}

/// The plaintext payload clients put inside an MLS application message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageContent {
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<MessageId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceParticipant {
    pub user_id: UserId,
    pub channel_id: ChannelId,
    pub muted: bool,
    pub deafened: bool,
    pub streaming: bool,
}
