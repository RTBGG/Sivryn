//! User profiles, presence and per-user settings.

use axum::Json;
use axum::extract::{Path, State};
use sivryn_protocol::api::{MeSettings, Relationship, UpdateMeRequest, UserProfile};
use sivryn_protocol::{GuildId, UserId};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::db;
use crate::error::{ApiError, ApiResult};

pub async fn relationship(state: &AppState, me: UserId, other: UserId) -> ApiResult<Relationship> {
    let (blocked, friend, incoming, outgoing): (bool, bool, bool, bool) = sqlx::query_as(
        "SELECT
             EXISTS (SELECT 1 FROM blocks WHERE user_id = $1 AND blocked_id = $2),
             EXISTS (SELECT 1 FROM friends WHERE user_id = $1 AND friend_id = $2),
             EXISTS (SELECT 1 FROM friend_requests WHERE from_id = $2 AND to_id = $1),
             EXISTS (SELECT 1 FROM friend_requests WHERE from_id = $1 AND to_id = $2)",
    )
    .bind(me.0)
    .bind(other.0)
    .fetch_one(&state.db)
    .await?;
    Ok(if blocked {
        Relationship::Blocked
    } else if friend {
        Relationship::Friend
    } else if incoming {
        Relationship::Incoming
    } else if outgoing {
        Relationship::Outgoing
    } else {
        Relationship::None
    })
}

/// A profile is visible to anyone who could see the user anyway: people
/// sharing a guild or DM, friends, and pending requests.
pub async fn profile(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<UserId>,
) -> ApiResult<Json<UserProfile>> {
    let user = db::user(&state.db, user_id).await?;
    let relationship = relationship(&state, auth.id, user_id).await?;
    let related = user_id == auth.id
        || relationship != Relationship::None
        || db::related_users(&state.db, auth.id).await?.contains(&user_id);
    if !related {
        return Err(ApiError::NotFound);
    }
    let mutual: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT a.guild_id FROM guild_members a JOIN guild_members b ON a.guild_id = b.guild_id
         WHERE a.user_id = $1 AND b.user_id = $2",
    )
    .bind(auth.id.0)
    .bind(user_id.0)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(UserProfile {
        user: user.into(),
        relationship,
        mutual_guilds: mutual.into_iter().map(|(g,)| GuildId(g)).collect(),
    }))
}

/// Which of the users you can see are online right now.
pub async fn presence(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Vec<UserId>>> {
    let mut online: Vec<UserId> =
        db::related_users(&state.db, auth.id).await?.into_iter().filter(|u| state.presence.is_online(*u)).collect();
    if state.presence.is_online(auth.id) {
        online.push(auth.id);
    }
    Ok(Json(online))
}

pub async fn settings(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<MeSettings>> {
    let user = db::user(&state.db, auth.id).await?;
    Ok(Json(MeSettings { allow_server_dms: user.allow_server_dms }))
}

pub async fn update_me(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<UpdateMeRequest>,
) -> ApiResult<Json<MeSettings>> {
    if let Some(allow) = req.allow_server_dms {
        sqlx::query("UPDATE users SET allow_server_dms = $2 WHERE id = $1")
            .bind(auth.id.0)
            .bind(allow)
            .execute(&state.db)
            .await?;
    }
    settings(State(state), auth).await
}
