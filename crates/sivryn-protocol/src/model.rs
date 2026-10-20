//! Domain objects as they appear on the wire.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::permissions::Permissions;
use crate::{ChannelId, DeviceId, FileId, GuildId, MessageId, RoleId, UserId};

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
    /// Icon image, served at `GET /api/v1/files/{id}`.
    #[serde(default)]
    pub icon_id: Option<FileId>,
    /// Voice channel idle members are moved to.
    #[serde(default)]
    pub afk_channel_id: Option<ChannelId>,
    /// Minutes without activity before moving to the AFK channel.
    #[serde(default = "default_afk_minutes")]
    pub afk_timeout_minutes: u32,
}

fn default_afk_minutes() -> u32 {
    15
}

/// AFK timeouts the server accepts, in minutes.
pub const AFK_TIMEOUTS: [u32; 5] = [1, 5, 15, 30, 60];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Text,
    Voice,
    /// A direct message between two users (text and calls).
    Dm,
    /// A group direct message (text and calls).
    Group,
}

impl ChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Voice => "voice",
            Self::Dm => "dm",
            Self::Group => "group",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "text" => Some(Self::Text),
            "voice" => Some(Self::Voice),
            "dm" => Some(Self::Dm),
            "group" => Some(Self::Group),
            _ => None,
        }
    }

    /// Direct and group messages live outside guilds.
    pub fn is_private(self) -> bool {
        matches!(self, Self::Dm | Self::Group)
    }

    /// Channels with a message history.
    pub fn has_messages(self) -> bool {
        !matches!(self, Self::Voice)
    }

    /// Channels you can join a call in.
    pub fn has_calls(self) -> bool {
        !matches!(self, Self::Text)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    /// `None` for direct and group messages.
    pub guild_id: Option<GuildId>,
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
    /// Edits and reactions are messages too, pointing at their `target_id`.
    #[serde(default)]
    pub kind: MessageKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<MessageId>,
    /// MLS epoch the message was encrypted in.
    pub epoch: u64,
    /// Base64 (URL-safe, unpadded) MLS ciphertext.
    pub ciphertext: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    #[default]
    Message,
    /// New content for `target_id` (only its author may edit).
    Edit,
    /// Adds or removes a reaction on `target_id`.
    Reaction,
}

impl MessageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Edit => "edit",
            Self::Reaction => "reaction",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "edit" => Self::Edit,
            "reaction" => Self::Reaction,
            _ => Self::Message,
        }
    }
}

/// The plaintext payload clients put inside an MLS application message.
/// Everything in here is end-to-end encrypted, including attachment keys,
/// link previews and reactions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageContent {
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<MessageId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub embeds: Vec<Embed>,
    /// Set on `MessageKind::Reaction` messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaction: Option<Reaction>,
}

/// An encrypted file. The server stores only the ciphertext at `file_id`;
/// `key` (base64, 32 bytes) decrypts it with XChaCha20-Poly1305 (the 24-byte
/// nonce is prepended to the ciphertext).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub file_id: FileId,
    pub name: String,
    pub mime: String,
    /// Plaintext size in bytes.
    pub size: u64,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Small preview (base64 PNG/JPEG) made by the sender, shown while loading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
}

/// A link preview, generated by the sender (receivers never contact the site).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Embed {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    /// Preview image, uploaded encrypted like an attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Attachment>,
    /// The link points straight at an image: show only the image.
    #[serde(default)]
    pub image_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaction {
    pub emoji: String,
    /// Removes the sender's earlier reaction with this emoji.
    #[serde(default)]
    pub remove: bool,
}
