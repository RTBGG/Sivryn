//! Domain objects as they appear on the wire.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::permissions::Permissions;
use crate::{CategoryId, ChannelId, DeviceId, FileId, GuildId, MessageId, RoleId, UserId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    /// Account name: unique, used to sign in and to add friends.
    pub username: String,
    /// Name shown to others (falls back to `username`).
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Profile picture, served at `GET /api/v1/files/{id}`.
    #[serde(default)]
    pub avatar_id: Option<FileId>,
}

impl User {
    /// The name to show: display name, or the account name.
    pub fn name(&self) -> &str {
        self.display_name.as_deref().filter(|n| !n.is_empty()).unwrap_or(&self.username)
    }
}

/// What a user shows others. `Invisible` looks offline to everyone else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    #[default]
    Online,
    /// Away.
    Idle,
    /// Busy: no notifications.
    Dnd,
    /// Shown as offline.
    Invisible,
}

impl UserStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Idle => "idle",
            Self::Dnd => "dnd",
            Self::Invisible => "invisible",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "idle" => Self::Idle,
            "dnd" => Self::Dnd,
            "invisible" => Self::Invisible,
            _ => Self::Online,
        }
    }
}

/// Which messages in a guild notify a member.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyLevel {
    #[default]
    All,
    Mentions,
    Nothing,
}

impl NotifyLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Mentions => "mentions",
            Self::Nothing => "nothing",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "mentions" => Self::Mentions,
            "nothing" => Self::Nothing,
            _ => Self::All,
        }
    }
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
    /// Opus bitrate for microphones in all of this guild's voice channels.
    #[serde(default = "default_audio_bitrate")]
    pub audio_bitrate: u32,
    /// Members who join later can read the channels' earlier messages
    /// (clients hand them the history keys). On for new guilds; servers that
    /// predate it don't send it, which reads as off.
    #[serde(default)]
    pub share_history: bool,
}

fn default_afk_minutes() -> u32 {
    15
}

pub fn default_audio_bitrate() -> u32 {
    128_000
}

/// Voice bitrates a guild can choose, in bits per second.
pub const AUDIO_BITRATES: [u32; 4] = [64_000, 128_000, 256_000, 320_000];

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

    /// Channels with a message history (voice channels have a chat too).
    pub fn has_messages(self) -> bool {
        true
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
    /// The category it is listed under (guild channels only).
    #[serde(default)]
    pub category_id: Option<CategoryId>,
    /// Permission changes for roles or members in this channel, applied
    /// after the category's (guild channels only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overwrites: Vec<PermissionOverwrite>,
}

/// A named group of channels in a guild's channel list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub id: CategoryId,
    pub guild_id: GuildId,
    pub name: String,
    pub position: i32,
    /// Permission changes for every channel in the category.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overwrites: Vec<PermissionOverwrite>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    pub id: RoleId,
    pub guild_id: GuildId,
    pub name: String,
    pub permissions: Permissions,
    /// Rank: higher roles come first and can manage lower ones. `@everyone` is 0.
    pub position: i32,
    /// Name color in the member list, as `0xRRGGBB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    /// Members with this role are listed in their own group.
    #[serde(default)]
    pub hoist: bool,
    /// The implicit role every member has.
    #[serde(default)]
    pub is_everyone: bool,
}

/// Who a permission overwrite applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum OverwriteTarget {
    Role(RoleId),
    Member(UserId),
}

/// Allows or denies permissions for a role or member in a channel or category.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionOverwrite {
    pub target: OverwriteTarget,
    pub allow: Permissions,
    pub deny: Permissions,
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
    /// The same content sealed with the channel's history key, for members
    /// who join later (key id, nonce and ciphertext, base64). Absent where
    /// the guild doesn't share history, and in DMs and groups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<String>,
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
    /// Hands the channel's history keys to the group (not shown; see
    /// [`MessageContent::history_keys`]).
    HistoryKeys,
}

impl MessageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Edit => "edit",
            Self::Reaction => "reaction",
            Self::HistoryKeys => "history_keys",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "edit" => Self::Edit,
            "reaction" => Self::Reaction,
            "history_keys" => Self::HistoryKeys,
            _ => Self::Message,
        }
    }
}

/// A channel's history key: messages are also sealed with it (see
/// [`Message::history`]), so members who join later can read what was
/// written before. It only travels inside MLS messages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryKey {
    /// Base64 (URL-safe, unpadded), 16 bytes; put in front of each sealed copy.
    pub id: String,
    /// Base64 (URL-safe, unpadded), 32 bytes (XChaCha20-Poly1305).
    pub key: String,
    /// Unix milliseconds; the newest key seals new messages.
    pub created_at: i64,
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
    /// Set on `MessageKind::HistoryKeys` messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history_keys: Vec<HistoryKey>,
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
