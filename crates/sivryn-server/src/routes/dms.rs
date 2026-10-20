//! Direct messages and group DMs: channels without a guild, with their own
//! member list. Both carry text and calls.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sivryn_protocol::api::{AddDmMemberRequest, DmChannel, OpenDmRequest, RenameDmRequest};
use sivryn_protocol::gateway::{Audience, Event};
use sivryn_protocol::model::ChannelKind;
use sivryn_protocol::{ChannelId, UserId};

use crate::AppState;
use crate::auth::AuthUser;
use crate::db::{self, ChannelRow};
use crate::error::{ApiError, ApiResult};

/// Largest group, including its creator (like Discord).
const MAX_GROUP_MEMBERS: usize = 10;

async fn dm_channel(state: &AppState, row: ChannelRow) -> ApiResult<DmChannel> {
    let ids = db::channel_member_ids(&state.db, ChannelId(row.id)).await?;
    Ok(DmChannel { members: db::users(&state.db, &ids).await?, channel: row.into() })
}

pub async fn list(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Vec<DmChannel>>> {
    // Most recently active first.
    let rows: Vec<ChannelRow> = sqlx::query_as(
        "SELECT c.* FROM channels c JOIN channel_members m ON m.channel_id = c.id
         WHERE m.user_id = $1
         ORDER BY GREATEST(c.created_at, COALESCE((SELECT max(created_at) FROM messages WHERE channel_id = c.id), c.created_at)) DESC",
    )
    .bind(auth.id.0)
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(dm_channel(&state, row).await?);
    }
    Ok(Json(out))
}

/// Whether `me` may start a conversation with `other`: not blocked, and
/// friends, or sharing a guild while `other` accepts DMs from guild members.
async fn may_message(state: &AppState, me: UserId, other: UserId) -> ApiResult<bool> {
    if db::blocked_either_way(&state.db, me, other).await? {
        return Ok(false);
    }
    if db::are_friends(&state.db, me, other).await? {
        return Ok(true);
    }
    let accepts = db::user(&state.db, other).await?.allow_server_dms;
    Ok(accepts && db::share_guild(&state.db, me, other).await?)
}

async fn create_channel(state: &AppState, kind: ChannelKind, members: &[UserId]) -> ApiResult<ChannelRow> {
    let mut tx = state.db.begin().await?;
    let row: ChannelRow = sqlx::query_as(
        "INSERT INTO channels (id, guild_id, name, kind, position, e2ee) VALUES ($1, NULL, '', $2, 0, TRUE) RETURNING *",
    )
    .bind(ChannelId::new().0)
    .bind(kind.as_str())
    .fetch_one(&mut *tx)
    .await?;
    for member in members {
        sqlx::query("INSERT INTO channel_members (channel_id, user_id) VALUES ($1, $2)")
            .bind(row.id)
            .bind(member.0)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    state.events.publish(
        Audience::Users(members.to_vec()),
        Event::DmUpdate { channel_id: ChannelId(row.id), added: members.to_vec(), removed: Vec::new() },
    );
    Ok(row)
}

/// Opens the DM with one user (reusing an existing one) or creates a group.
pub async fn open(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<OpenDmRequest>,
) -> ApiResult<(StatusCode, Json<DmChannel>)> {
    let mut others: Vec<UserId> = req.user_ids.into_iter().filter(|u| *u != auth.id).collect();
    others.sort();
    others.dedup();
    if others.is_empty() {
        return Err(ApiError::BadRequest("pick at least one other user".into()));
    }
    if others.len() + 1 > MAX_GROUP_MEMBERS {
        return Err(ApiError::BadRequest(format!("groups have at most {MAX_GROUP_MEMBERS} members")));
    }
    for other in &others {
        db::user(&state.db, *other).await.map_err(|_| ApiError::NotFound)?;
        if !may_message(&state, auth.id, *other).await? {
            return Err(ApiError::Forbidden);
        }
    }

    if let [other] = others.as_slice() {
        let existing: Option<ChannelRow> = sqlx::query_as(
            "SELECT c.* FROM channels c
             JOIN channel_members a ON a.channel_id = c.id AND a.user_id = $1
             JOIN channel_members b ON b.channel_id = c.id AND b.user_id = $2
             WHERE c.kind = 'dm' LIMIT 1",
        )
        .bind(auth.id.0)
        .bind(other.0)
        .fetch_optional(&state.db)
        .await?;
        if let Some(row) = existing {
            return Ok((StatusCode::OK, Json(dm_channel(&state, row).await?)));
        }
        let row = create_channel(&state, ChannelKind::Dm, &[auth.id, *other]).await?;
        return Ok((StatusCode::CREATED, Json(dm_channel(&state, row).await?)));
    }

    let mut members = vec![auth.id];
    members.extend(others);
    let row = create_channel(&state, ChannelKind::Group, &members).await?;
    Ok((StatusCode::CREATED, Json(dm_channel(&state, row).await?)))
}

/// Adds someone. A 1:1 DM turns into a new group with all three members,
/// so the original conversation stays private.
pub async fn add_member(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
    Json(req): Json<AddDmMemberRequest>,
) -> ApiResult<(StatusCode, Json<DmChannel>)> {
    let channel = db::channel(&state.db, channel_id).await?;
    if !channel.kind().is_private() || !db::is_channel_member(&state.db, channel_id, auth.id).await? {
        return Err(ApiError::NotFound);
    }
    let members = db::channel_member_ids(&state.db, channel_id).await?;
    if members.contains(&req.user_id) {
        return Err(ApiError::Conflict("already a member".into()));
    }
    if members.len() + 1 > MAX_GROUP_MEMBERS {
        return Err(ApiError::BadRequest(format!("groups have at most {MAX_GROUP_MEMBERS} members")));
    }
    if !may_message(&state, auth.id, req.user_id).await? {
        return Err(ApiError::Forbidden);
    }

    if channel.kind() == ChannelKind::Dm {
        let mut all = members.clone();
        all.push(req.user_id);
        let row = create_channel(&state, ChannelKind::Group, &all).await?;
        return Ok((StatusCode::CREATED, Json(dm_channel(&state, row).await?)));
    }

    sqlx::query("INSERT INTO channel_members (channel_id, user_id) VALUES ($1, $2)")
        .bind(channel_id.0)
        .bind(req.user_id.0)
        .execute(&state.db)
        .await?;
    let mut audience = members;
    audience.push(req.user_id);
    state.events.publish(
        Audience::Users(audience),
        Event::DmUpdate { channel_id, added: vec![req.user_id], removed: Vec::new() },
    );
    Ok((StatusCode::OK, Json(dm_channel(&state, channel).await?)))
}

/// Leaves a group. (1:1 DMs can't be left; they just stay in the list.)
pub async fn leave(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
) -> ApiResult<StatusCode> {
    let channel = db::channel(&state.db, channel_id).await?;
    if channel.kind() != ChannelKind::Group {
        return Err(ApiError::BadRequest("only groups can be left".into()));
    }
    let members = db::channel_member_ids(&state.db, channel_id).await?;
    if !members.contains(&auth.id) {
        return Err(ApiError::NotFound);
    }
    sqlx::query("DELETE FROM channel_members WHERE channel_id = $1 AND user_id = $2")
        .bind(channel_id.0)
        .bind(auth.id.0)
        .execute(&state.db)
        .await?;
    // The last one out deletes the group with its messages and files.
    if members.len() == 1 {
        sqlx::query("DELETE FROM channels WHERE id = $1").bind(channel_id.0).execute(&state.db).await?;
    }
    state
        .events
        .publish(Audience::Users(members), Event::DmUpdate { channel_id, added: Vec::new(), removed: vec![auth.id] });
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rename(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
    Json(req): Json<RenameDmRequest>,
) -> ApiResult<Json<DmChannel>> {
    let channel = db::channel(&state.db, channel_id).await?;
    if channel.kind() != ChannelKind::Group || !db::is_channel_member(&state.db, channel_id, auth.id).await? {
        return Err(ApiError::NotFound);
    }
    let name: String = req.name.trim().chars().take(100).collect();
    let row: ChannelRow = sqlx::query_as("UPDATE channels SET name = $2 WHERE id = $1 RETURNING *")
        .bind(channel_id.0)
        .bind(&name)
        .fetch_one(&state.db)
        .await?;
    let members = db::channel_member_ids(&state.db, channel_id).await?;
    state
        .events
        .publish(Audience::Users(members), Event::DmUpdate { channel_id, added: Vec::new(), removed: Vec::new() });
    Ok(Json(dm_channel(&state, row).await?))
}
