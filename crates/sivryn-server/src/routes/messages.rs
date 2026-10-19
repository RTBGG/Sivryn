use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use sivryn_protocol::api::{MessagePage, MessageQuery, SendMessageRequest};
use sivryn_protocol::gateway::Event;
use sivryn_protocol::model::{ChannelKind, Message};
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{ChannelId, GuildId, MessageId};

use crate::AppState;
use crate::auth::AuthUser;
use crate::db::{self, MessageRow};
use crate::error::{ApiError, ApiResult};

const MAX_CIPHERTEXT_LEN: usize = 64 * 1024;
const DEFAULT_PAGE: u32 = 50;
const MAX_PAGE: u32 = 100;

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
    Query(q): Query<MessageQuery>,
) -> ApiResult<Json<MessagePage>> {
    db::require_channel(&state.db, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    let limit = q.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE) as i64;

    // UUIDv7 sorts by time: `id < before` pages backwards through history,
    // `id > after` walks forwards from the last message a client has seen.
    let rows: Vec<MessageRow> = if let Some(after) = q.after {
        sqlx::query_as(
            "SELECT * FROM messages
             WHERE channel_id = $1 AND id > $2 AND ($3::UUID IS NULL OR id < $3)
             ORDER BY id ASC LIMIT $4",
        )
        .bind(channel_id.0)
        .bind(after.0)
        .bind(q.before.map(|id| id.0))
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as(
            "SELECT * FROM messages
             WHERE channel_id = $1 AND ($2::UUID IS NULL OR id < $2)
             ORDER BY id DESC LIMIT $3",
        )
        .bind(channel_id.0)
        .bind(q.before.map(|id| id.0))
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    };

    Ok(Json(MessagePage { messages: rows.into_iter().map(Into::into).collect() }))
}

pub async fn send(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
    Json(req): Json<SendMessageRequest>,
) -> ApiResult<(StatusCode, Json<Message>)> {
    let channel = db::require_channel(&state.db, channel_id, auth.id, Permissions::SEND_MESSAGES).await?;
    if ChannelKind::parse(&channel.kind) != Some(ChannelKind::Text) {
        return Err(ApiError::BadRequest("not a text channel".into()));
    }
    if req.ciphertext.is_empty() || req.ciphertext.len() > MAX_CIPHERTEXT_LEN {
        return Err(ApiError::BadRequest("ciphertext is empty or too large".into()));
    }
    db::require_own_device(&state.db, req.device_id, auth.id).await?;

    let row: MessageRow = sqlx::query_as(
        "INSERT INTO messages (id, channel_id, author_id, sender_device_id, epoch, ciphertext)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING *",
    )
    .bind(MessageId::new().0)
    .bind(channel_id.0)
    .bind(auth.id.0)
    .bind(req.device_id.0)
    .bind(req.epoch as i64)
    .bind(&req.ciphertext)
    .fetch_one(&state.db)
    .await?;

    let message: Message = row.into();
    state.events.publish(Event::MessageCreate { guild_id: GuildId(channel.guild_id), message: message.clone() });
    Ok((StatusCode::CREATED, Json(message)))
}
