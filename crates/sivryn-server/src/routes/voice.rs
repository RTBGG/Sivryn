//! Voice channels map 1:1 to LiveKit rooms named after the channel ID.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use livekit_api::access_token::{AccessToken, VideoGrants};
use sivryn_protocol::ChannelId;
use sivryn_protocol::api::VoiceJoinResponse;
use sivryn_protocol::model::ChannelKind;
use sivryn_protocol::permissions::Permissions;

use crate::AppState;
use crate::auth::AuthUser;
use crate::db;
use crate::error::{ApiError, ApiResult};

const TOKEN_TTL: Duration = Duration::from_secs(10 * 60);

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
