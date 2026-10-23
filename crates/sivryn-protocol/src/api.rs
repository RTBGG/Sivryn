//! REST request and response bodies. Routes live under `/api/v1`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::media::{CameraLimits, ScreenShareLimits};
use chrono::{DateTime, Utc};

use crate::model::{Category, Channel, ChannelKind, Guild, Message, MessageKind, NotifyLevel, Role, User, UserStatus};
use crate::permissions::Permissions;
use crate::{CategoryId, ChannelId, DeviceId, FileId, GuildId, MessageId, RoleId, UserId};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
}

/// `GET /api/v1/instance`: public information shown before login.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub name: String,
    pub version: String,
    pub registration: RegistrationMode,
    pub oidc_enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationMode {
    Open,
    InviteOnly,
    Closed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub invite_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    /// Opaque bearer token. Send as `Authorization: Bearer <token>`.
    pub token: String,
    pub user: User,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateGuildRequest {
    pub name: String,
    /// Names for the default categories, in the creator's language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_category: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    pub kind: ChannelKind,
    #[serde(default)]
    pub e2ee: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_id: Option<CategoryId>,
}

/// `PATCH /api/v1/channels/{id}`
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UpdateChannelRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `Some(None)` moves the channel out of its category.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub category_id: Option<Option<CategoryId>>,
}

/// `POST /api/v1/guilds/{id}/categories` and `PATCH /api/v1/categories/{id}`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CategoryRequest {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuildWithChannels {
    pub guild: Guild,
    pub channels: Vec<Channel>,
    #[serde(default)]
    pub categories: Vec<Category>,
    /// The requesting user's permissions in this guild.
    #[serde(default = "Permissions::empty")]
    pub permissions: Permissions,
    /// The requesting user's name in this guild, if set.
    #[serde(default)]
    pub nick: Option<String>,
    /// Which messages here notify the requesting user.
    #[serde(default)]
    pub notify_level: NotifyLevel,
    /// All roles, highest first (`@everyone` last).
    #[serde(default)]
    pub roles: Vec<Role>,
    /// The requesting user's permissions per channel (after overwrites).
    /// Channels they can't see aren't listed at all.
    #[serde(default)]
    pub channel_permissions: HashMap<ChannelId, Permissions>,
}

/// `GET /api/v1/guilds/{id}/members`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuildMember {
    pub user: User,
    /// Name in this guild, overriding the display name.
    #[serde(default)]
    pub nick: Option<String>,
    /// Roles besides `@everyone`.
    #[serde(default)]
    pub roles: Vec<RoleId>,
}

/// `PATCH /api/v1/guilds/{id}/members/@me`
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UpdateMemberRequest {
    /// `Some(None)` clears the name in this guild.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub nick: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_level: Option<NotifyLevel>,
}

/// `GET /api/v1/users/@me/read-states`: how far each channel has been read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadState {
    pub channel_id: ChannelId,
    pub last_read_id: Option<MessageId>,
    /// Messages from others since then (capped at 100).
    pub unread: u32,
}

/// `PUT /api/v1/channels/{id}/read`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AckRequest {
    pub message_id: MessageId,
}

/// `GET /api/v1/users/@me/mutes`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelMute {
    pub channel_id: ChannelId,
    /// `None`: until unmuted.
    pub until: Option<DateTime<Utc>>,
}

/// `PUT /api/v1/channels/{id}/mute`
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MuteRequest {
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
}

/// `GET /api/v1/presence`: status of online users you know (friends, guild
/// and DM members). Missing means offline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPresence {
    pub user_id: UserId,
    pub status: UserStatus,
}

/// `POST /api/v1/channels/{id}/messages`: send an MLS-encrypted message.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub device_id: DeviceId,
    pub epoch: u64,
    pub ciphertext: String,
    /// See [`crate::model::Message::history`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<String>,
    #[serde(default)]
    pub kind: MessageKind,
    /// Required for edits and reactions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<MessageId>,
}

/// `GET /api/v1/channels/{id}/messages?before=&after=&limit=`
///
/// Without `after` the newest messages come first and `before` pages back
/// through history. With `after` the oldest messages newer than it come first,
/// which is how clients catch up after being offline.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MessageQuery {
    pub before: Option<MessageId>,
    pub after: Option<MessageId>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessagePage {
    pub messages: Vec<Message>,
}

// --- MLS delivery service -------------------------------------------------

/// `POST /api/v1/devices`: register a device and its MLS key packages.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterDeviceRequest {
    pub name: String,
    /// Base64 serialized MLS key packages (one-time use each).
    pub key_packages: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterDeviceResponse {
    pub device_id: DeviceId,
}

/// `GET /api/v1/devices/{id}/key-packages`: how many unclaimed key packages
/// the server still holds for this device, so clients know when to top up.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct KeyPackageCount {
    pub count: u32,
}

/// `POST /api/v1/users/{id}/key-packages/claim`: one key package per device.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaimedKeyPackage {
    pub user_id: UserId,
    pub device_id: DeviceId,
    pub key_package: String,
}

/// `POST /api/v1/channels/{id}/mls/commits`: a commit plus optional welcome
/// for newly added devices. The server orders commits per channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MlsCommitRequest {
    pub device_id: DeviceId,
    pub epoch: u64,
    pub commit: String,
    #[serde(default)]
    pub welcome: Option<MlsWelcome>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MlsWelcome {
    pub recipients: Vec<DeviceId>,
    pub welcome: String,
    /// Ratchet tree, so joiners don't need the server to reconstruct it.
    pub ratchet_tree: String,
}

/// `GET /api/v1/channels/{id}/mls/commits?since=<epoch>`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredCommit {
    pub epoch: u64,
    pub commit: String,
}

/// `GET /api/v1/devices/{id}/welcomes`: returned once, then deleted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingWelcome {
    pub channel_id: ChannelId,
    pub welcome: MlsWelcome,
}

// --- Voice ----------------------------------------------------------------

/// `POST /api/v1/channels/{id}/voice/join` response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VoiceJoinResponse {
    pub channel_id: ChannelId,
    /// LiveKit URL clients connect to (usually `wss://<domain>`).
    pub livekit_url: String,
    /// Short-lived LiveKit access token for this channel's room.
    pub token: String,
    pub screen_share: ScreenShareLimits,
    pub camera: CameraLimits,
    pub e2ee: bool,
    /// Microphone bitrate (bits per second) set by the guild.
    #[serde(default = "crate::model::default_audio_bitrate")]
    pub audio_bitrate: u32,
}

// --- Voice presence ---------------------------------------------------------

/// `GET /api/v1/guilds/{id}/voice`: who is connected to each voice channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceChannelState {
    pub channel_id: ChannelId,
    pub participants: Vec<VoiceParticipant>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceParticipant {
    pub user_id: UserId,
    pub muted: bool,
    /// Turned off their sound in Sivryn.
    #[serde(default)]
    pub deafened: bool,
    pub camera: bool,
    pub screen_sharing: bool,
}

// --- Users, friends and blocks -----------------------------------------------

/// How the requesting user relates to another user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relationship {
    #[default]
    None,
    Friend,
    /// They sent you a friend request.
    Incoming,
    /// You sent them a friend request.
    Outgoing,
    /// You blocked them.
    Blocked,
}

/// `GET /api/v1/users/{id}`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserProfile {
    pub user: User,
    pub relationship: Relationship,
    pub mutual_guilds: Vec<GuildId>,
    #[serde(default)]
    pub bio: String,
    #[serde(default)]
    pub banner_id: Option<FileId>,
    /// `None` while offline (or invisible).
    #[serde(default)]
    pub status: Option<UserStatus>,
}

/// `GET /api/v1/friends`
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FriendsResponse {
    pub friends: Vec<User>,
    pub incoming: Vec<User>,
    pub outgoing: Vec<User>,
    pub blocked: Vec<User>,
}

/// `POST /api/v1/friends/requests`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FriendRequest {
    pub username: String,
}

/// `PATCH /api/v1/users/@me`
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UpdateMeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_server_dms: Option<bool>,
    /// `Some(None)` goes back to showing the account name.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub display_name: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<UserStatus>,
}

/// `GET /api/v1/users/@me/settings`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeSettings {
    pub allow_server_dms: bool,
    #[serde(default)]
    pub status: UserStatus,
    #[serde(default)]
    pub bio: String,
    #[serde(default)]
    pub banner_id: Option<FileId>,
}

// --- Direct and group messages ------------------------------------------------

/// `GET /api/v1/dms`: a DM or group with its members.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DmChannel {
    pub channel: Channel,
    pub members: Vec<User>,
}

/// `POST /api/v1/dms`: one user opens (or reuses) a DM, several make a group.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenDmRequest {
    pub user_ids: Vec<UserId>,
}

/// `POST /api/v1/dms/{id}/members`. Adding someone to a 1:1 DM creates a new
/// group with all three, like Discord.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddDmMemberRequest {
    pub user_id: UserId,
}

/// `PATCH /api/v1/dms/{id}`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenameDmRequest {
    pub name: String,
}

// --- Guild management -----------------------------------------------------------

/// `PATCH /api/v1/guilds/{id}`
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UpdateGuildRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `Some(None)` clears the AFK channel.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub afk_channel_id: Option<Option<ChannelId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub afk_timeout_minutes: Option<u32>,
    /// One of [`crate::model::AUDIO_BITRATES`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_bitrate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_history: Option<bool>,
}

mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, T: Serialize>(v: &Option<Option<T>>, s: S) -> Result<S::Ok, S::Error> {
        v.as_ref().expect("skipped when None").serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(d).map(Some)
    }
}

/// `POST /api/v1/guilds/{id}/roles` and `PATCH /api/v1/roles/{id}`
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RoleRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Permissions>,
    /// `Some(None)` removes the color.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub color: Option<Option<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hoist: Option<bool>,
}

/// `PUT /api/v1/guilds/{id}/roles/order`: roles from highest to lowest,
/// without `@everyone`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoleOrderRequest {
    pub roles: Vec<RoleId>,
}

/// `PUT /api/v1/guilds/{id}/layout`: the whole channel list after a drag.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayoutRequest {
    /// Categories from top to bottom.
    pub categories: Vec<CategoryId>,
    /// Every channel with its category, in order within that category.
    pub channels: Vec<ChannelPlacement>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelPlacement {
    pub id: ChannelId,
    #[serde(default)]
    pub category_id: Option<CategoryId>,
}

/// `POST /api/v1/guilds/{id}/voice/move`: moves someone to another voice channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MoveMemberRequest {
    pub user_id: UserId,
    pub channel_id: ChannelId,
}

/// `GET /api/v1/guilds/{id}/bans`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ban {
    pub user: User,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `PUT /api/v1/guilds/{id}/bans/{user}`
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BanRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// --- Files --------------------------------------------------------------------------

/// Response of `POST /api/v1/channels/{id}/attachments` (body: the encrypted
/// bytes) and `PUT /api/v1/guilds/{id}/icon` (body: the image).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct UploadedFile {
    pub id: FileId,
    pub size: u64,
}
