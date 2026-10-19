//! Voice channels map 1:1 to LiveKit rooms named after the channel ID.

use std::sync::OnceLock;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use livekit_api::access_token::{AccessToken, VideoGrants};
use serde::Deserialize;
use sivryn_protocol::api::{VoiceChannelState, VoiceJoinResponse, VoiceParticipant};
use sivryn_protocol::model::ChannelKind;
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{ChannelId, GuildId, UserId};

use crate::AppState;
use crate::auth::AuthUser;
use crate::db;
use crate::error::{ApiError, ApiResult};

const TOKEN_TTL: Duration = Duration::from_secs(10 * 60);
const LIVEKIT_API_TIMEOUT: Duration = Duration::from_secs(3);

pub async fn join(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
) -> ApiResult<Json<VoiceJoinResponse>> {
    let channel = db::require_channel(&state.db, channel_id, auth.id, Permissions::CONNECT).await?;
    if ChannelKind::parse(&channel.kind) != Some(ChannelKind::Voice) {
        return Err(ApiError::BadRequest("not a voice channel".into()));
    }

    let perms = db::guild_permissions(&state.db, channel.guild_id.into(), auth.id).await?.ok_or(ApiError::NotFound)?;
    let user = db::user(&state.db, auth.id).await?;

    // Guild permissions decide which track sources LiveKit lets this user publish.
    let mut sources = Vec::new();
    if perms.allows(Permissions::SPEAK) {
        sources.push("microphone".to_owned());
    }
    if perms.allows(Permissions::VIDEO) {
        sources.push("camera".to_owned());
    }
    if perms.allows(Permissions::SCREEN_SHARE) {
        sources.extend(["screen_share".to_owned(), "screen_share_audio".to_owned()]);
    }

    let grants = VideoGrants {
        room_join: true,
        room: channel_id.to_string(),
        can_publish: Some(!sources.is_empty()),
        can_publish_sources: sources,
        can_subscribe: Some(true),
        can_publish_data: Some(true),
        ..Default::default()
    };

    let token = AccessToken::with_api_key(&state.config.livekit.api_key, &state.config.livekit.api_secret)
        .with_identity(&auth.id.to_string())
        .with_name(user.display_name.as_deref().unwrap_or(&user.username))
        .with_ttl(TOKEN_TTL)
        .with_grants(grants)
        .to_jwt()
        .map_err(|e| anyhow::anyhow!("signing LiveKit token: {e}"))?;

    Ok(Json(VoiceJoinResponse {
        channel_id,
        livekit_url: state.config.livekit.public_url.clone(),
        token,
        screen_share: state.config.screen_share.clone(),
        camera: state.config.camera.clone(),
        e2ee: channel.e2ee,
    }))
}

/// Lists who is connected to each voice channel of a guild, straight from
/// LiveKit (rooms are named after channel IDs). A LiveKit outage shows up as
/// empty channels rather than an error, so the rest of the UI keeps working.
pub async fn guild_states(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
) -> ApiResult<Json<Vec<VoiceChannelState>>> {
    db::require(&state.db, guild_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    let channels: Vec<(uuid::Uuid,)> =
        sqlx::query_as("SELECT id FROM channels WHERE guild_id = $1 AND kind = 'voice' ORDER BY position, id")
            .bind(guild_id.0)
            .fetch_all(&state.db)
            .await?;

    let lookups = channels.into_iter().map(|(id,)| {
        let state = state.clone();
        async move {
            let channel_id = ChannelId(id);
            let participants = match list_participants(&state, channel_id).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, %channel_id, "could not list LiveKit participants");
                    Vec::new()
                }
            };
            VoiceChannelState { channel_id, participants }
        }
    });
    Ok(Json(futures::future::join_all(lookups).await))
}

#[derive(Deserialize)]
struct ListParticipantsResponse {
    #[serde(default)]
    participants: Vec<ParticipantInfo>,
}

#[derive(Deserialize)]
struct ParticipantInfo {
    identity: String,
    #[serde(default)]
    tracks: Vec<TrackInfo>,
}

#[derive(Deserialize)]
struct TrackInfo {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    muted: bool,
}

/// Calls LiveKit's Twirp `RoomService.ListParticipants` with JSON encoding.
async fn list_participants(state: &AppState, channel: ChannelId) -> anyhow::Result<Vec<VoiceParticipant>> {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    let http = HTTP
        .get_or_init(|| reqwest::Client::builder().timeout(LIVEKIT_API_TIMEOUT).build().expect("HTTP client builds"));

    let lk = &state.config.livekit;
    let room = channel.to_string();
    let token = AccessToken::with_api_key(&lk.api_key, &lk.api_secret)
        .with_ttl(Duration::from_secs(60))
        .with_grants(VideoGrants { room_admin: true, room: room.clone(), ..Default::default() })
        .to_jwt()?;
    let resp = http
        .post(format!("{}/twirp/livekit.RoomService/ListParticipants", lk.api_url))
        .bearer_auth(token)
        .json(&serde_json::json!({ "room": room }))
        .send()
        .await?;
    // A room that doesn't exist yet simply has nobody in it.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Vec::new());
    }
    let body: ListParticipantsResponse = resp.error_for_status()?.json().await?;

    Ok(body
        .participants
        .into_iter()
        .filter_map(|p| {
            let user_id = UserId(p.identity.parse().ok()?);
            let has = |source: &str| p.tracks.iter().any(|t| t.source.as_deref() == Some(source) && !t.muted);
            Some(VoiceParticipant {
                user_id,
                muted: !has("MICROPHONE"),
                camera: has("CAMERA"),
                screen_sharing: has("SCREEN_SHARE"),
            })
        })
        .collect())
}
