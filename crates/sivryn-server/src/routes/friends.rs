//! Friend requests, friendships and blocks.
//!
//! A request becomes a friendship when the other user accepts it, or right
//! away if they had already asked first. Blocking removes any friendship and
//! pending requests in both directions and stops new requests and DMs.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sivryn_protocol::UserId;
use sivryn_protocol::api::{FriendRequest, FriendsResponse};
use sivryn_protocol::gateway::{Audience, Event};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::db::{self, UserRow};
use crate::error::{ApiError, ApiResult};

async fn user_list(state: &AppState, sql: &'static str, user: UserId) -> ApiResult<Vec<sivryn_protocol::model::User>> {
    let rows: Vec<UserRow> = sqlx::query_as(sql).bind(user.0).fetch_all(&state.db).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn list(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<FriendsResponse>> {
    Ok(Json(FriendsResponse {
        friends: user_list(
            &state,
            "SELECT u.* FROM users u JOIN friends f ON f.friend_id = u.id WHERE f.user_id = $1 ORDER BY lower(u.username)",
            auth.id,
        )
        .await?,
        incoming: user_list(
            &state,
            "SELECT u.* FROM users u JOIN friend_requests r ON r.from_id = u.id WHERE r.to_id = $1 ORDER BY r.created_at",
            auth.id,
        )
        .await?,
        outgoing: user_list(
            &state,
            "SELECT u.* FROM users u JOIN friend_requests r ON r.to_id = u.id WHERE r.from_id = $1 ORDER BY r.created_at",
            auth.id,
        )
        .await?,
        blocked: user_list(
            &state,
            "SELECT u.* FROM users u JOIN blocks b ON b.blocked_id = u.id WHERE b.user_id = $1 ORDER BY lower(u.username)",
            auth.id,
        )
        .await?,
    }))
}

fn notify(state: &AppState, users: Vec<UserId>) {
    state.events.publish(Audience::Users(users), Event::RelationshipsChanged);
}

async fn make_friends(state: &AppState, a: UserId, b: UserId) -> ApiResult<()> {
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM friend_requests WHERE (from_id = $1 AND to_id = $2) OR (from_id = $2 AND to_id = $1)")
        .bind(a.0)
        .bind(b.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO friends (user_id, friend_id) VALUES ($1, $2), ($2, $1) ON CONFLICT DO NOTHING")
        .bind(a.0)
        .bind(b.0)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Sends a friend request by username (or accepts theirs, if pending).
pub async fn request(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<FriendRequest>,
) -> ApiResult<StatusCode> {
    let target: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE lower(username) = lower($1)")
        .bind(req.username.trim())
        .fetch_optional(&state.db)
        .await?;
    let target = UserId(target.ok_or(ApiError::NotFound)?.0);
    if target == auth.id {
        return Err(ApiError::BadRequest("you can't add yourself".into()));
    }
    // Don't reveal a block: it just looks like the user doesn't exist.
    if db::blocked_either_way(&state.db, auth.id, target).await? {
        return Err(ApiError::NotFound);
    }
    if db::are_friends(&state.db, auth.id, target).await? {
        return Err(ApiError::Conflict("already friends".into()));
    }
    let (they_asked,): (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM friend_requests WHERE from_id = $1 AND to_id = $2)")
            .bind(target.0)
            .bind(auth.id.0)
            .fetch_one(&state.db)
            .await?;
    if they_asked {
        make_friends(&state, auth.id, target).await?;
    } else {
        sqlx::query("INSERT INTO friend_requests (from_id, to_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(auth.id.0)
            .bind(target.0)
            .execute(&state.db)
            .await?;
    }
    notify(&state, vec![auth.id, target]);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn accept(State(state): State<AppState>, auth: AuthUser, Path(from): Path<UserId>) -> ApiResult<StatusCode> {
    let (pending,): (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM friend_requests WHERE from_id = $1 AND to_id = $2)")
            .bind(from.0)
            .bind(auth.id.0)
            .fetch_one(&state.db)
            .await?;
    if !pending {
        return Err(ApiError::NotFound);
    }
    make_friends(&state, auth.id, from).await?;
    notify(&state, vec![auth.id, from]);
    Ok(StatusCode::NO_CONTENT)
}

/// Declines an incoming request or cancels an outgoing one.
pub async fn cancel(State(state): State<AppState>, auth: AuthUser, Path(other): Path<UserId>) -> ApiResult<StatusCode> {
    let removed =
        sqlx::query("DELETE FROM friend_requests WHERE (from_id = $1 AND to_id = $2) OR (from_id = $2 AND to_id = $1)")
            .bind(auth.id.0)
            .bind(other.0)
            .execute(&state.db)
            .await?;
    if removed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    notify(&state, vec![auth.id, other]);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(friend): Path<UserId>,
) -> ApiResult<StatusCode> {
    let removed =
        sqlx::query("DELETE FROM friends WHERE (user_id = $1 AND friend_id = $2) OR (user_id = $2 AND friend_id = $1)")
            .bind(auth.id.0)
            .bind(friend.0)
            .execute(&state.db)
            .await?;
    if removed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    notify(&state, vec![auth.id, friend]);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn block(State(state): State<AppState>, auth: AuthUser, Path(target): Path<UserId>) -> ApiResult<StatusCode> {
    if target == auth.id {
        return Err(ApiError::BadRequest("you can't block yourself".into()));
    }
    db::user(&state.db, target).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("INSERT INTO blocks (user_id, blocked_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(auth.id.0)
        .bind(target.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM friends WHERE (user_id = $1 AND friend_id = $2) OR (user_id = $2 AND friend_id = $1)")
        .bind(auth.id.0)
        .bind(target.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM friend_requests WHERE (from_id = $1 AND to_id = $2) OR (from_id = $2 AND to_id = $1)")
        .bind(auth.id.0)
        .bind(target.0)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    notify(&state, vec![auth.id, target]);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unblock(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(target): Path<UserId>,
) -> ApiResult<StatusCode> {
    let removed = sqlx::query("DELETE FROM blocks WHERE user_id = $1 AND blocked_id = $2")
        .bind(auth.id.0)
        .bind(target.0)
        .execute(&state.db)
        .await?;
    if removed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    notify(&state, vec![auth.id]);
    Ok(StatusCode::NO_CONTENT)
}
