//! REST request and response bodies. Routes live under `/api/v1`.

use serde::{Deserialize, Serialize};

use crate::media::{CameraLimits, ScreenShareLimits};
use crate::model::{Channel, ChannelKind, Guild, Message, User};
use crate::{ChannelId, DeviceId, MessageId, UserId};

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
}

/// `POST /api/v1/channels/{id}/messages`: send an MLS-encrypted message.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub device_id: DeviceId,
    pub epoch: u64,
    pub ciphertext: String,
}

/// `GET /api/v1/channels/{id}/messages?before=&limit=`
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MessageQuery {
    pub before: Option<MessageId>,
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
