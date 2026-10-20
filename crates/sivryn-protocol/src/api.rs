//! REST request and response bodies. Routes live under `/api/v1`.

use serde::{Deserialize, Serialize};

use crate::media::{CameraLimits, ScreenShareLimits};
use crate::model::{Channel, ChannelKind, Guild, Message, MessageKind, User};
use crate::permissions::Permissions;
use crate::{ChannelId, DeviceId, FileId, GuildId, MessageId, UserId};

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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    pub kind: ChannelKind,
    #[serde(default)]
    pub e2ee: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GuildWithChannels {
    pub guild: Guild,
    pub channels: Vec<Channel>,
    /// The requesting user's permissions in this guild.
    #[serde(default = "Permissions::empty")]
    pub permissions: Permissions,
}

/// `POST /api/v1/channels/{id}/messages`: send an MLS-encrypted message.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub device_id: DeviceId,
    pub epoch: u64,
    pub ciphertext: String,
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
}

/// `GET /api/v1/users/@me/settings`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeSettings {
    pub allow_server_dms: bool,
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
