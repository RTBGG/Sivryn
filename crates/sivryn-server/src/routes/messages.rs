use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use sivryn_protocol::api::{MessagePage, MessageQuery, SendMessageRequest};
use sivryn_protocol::gateway::Event;
use sivryn_protocol::model::{Message, MessageKind};
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{ChannelId, MessageId};
use uuid::Uuid;

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
    if !channel.kind().has_messages() {
        return Err(ApiError::BadRequest("this channel has no messages".into()));
    }
    if req.ciphertext.is_empty() || req.ciphertext.len() > MAX_CIPHERTEXT_LEN {
        return Err(ApiError::BadRequest("ciphertext is empty or too large".into()));
    }
    if req.history.as_ref().is_some_and(|h| h.is_empty() || h.len() > MAX_CIPHERTEXT_LEN) {
        return Err(ApiError::BadRequest("history copy is empty or too large".into()));
    }
    db::require_own_device(&state.db, req.device_id, auth.id).await?;

    // Edits and reactions must point at a normal message in this channel;
    // only its author may edit it.
    match (req.kind, req.target_id) {
        (MessageKind::Message | MessageKind::HistoryKeys, None) => {}
        (MessageKind::Message | MessageKind::HistoryKeys, Some(_)) => {
            return Err(ApiError::BadRequest("messages have no target".into()));
        }
        (_, None) => return Err(ApiError::BadRequest("edits and reactions need a target".into())),
        (kind, Some(target)) => {
            let row: Option<(Uuid, String)> =
                sqlx::query_as("SELECT author_id, kind FROM messages WHERE id = $1 AND channel_id = $2")
                    .bind(target.0)
                    .bind(channel_id.0)
                    .fetch_optional(&state.db)
                    .await?;
            let (author, target_kind) = row.ok_or(ApiError::NotFound)?;
            if MessageKind::parse(&target_kind) != MessageKind::Message {
                return Err(ApiError::BadRequest("target must be a message".into()));
            }
            if kind == MessageKind::Edit && author != auth.id.0 {
                return Err(ApiError::Forbidden);
            }
        }
    }

    let row: MessageRow = sqlx::query_as(
        "INSERT INTO messages (id, channel_id, author_id, sender_device_id, epoch, ciphertext, kind, target_id, history)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING *",
    )
    .bind(MessageId::new().0)
    .bind(channel_id.0)
    .bind(auth.id.0)
    .bind(req.device_id.0)
    .bind(req.epoch as i64)
    .bind(&req.ciphertext)
    .bind(req.kind.as_str())
    .bind(req.target_id.map(|t| t.0))
    .bind(req.history.as_deref())
    .fetch_one(&state.db)
    .await?;

    let message: Message = row.into();
    let audience = db::channel_audience(&state.db, &channel).await?;
    state.events.publish(audience, Event::MessageCreate { message: message.clone() });
    Ok((StatusCode::CREATED, Json(message)))
}

/// Deletes a message with its edits and reactions. Authors may delete their
/// own messages; in guilds, `MANAGE_MESSAGES` may delete anyone's.
pub async fn delete(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((channel_id, message_id)): Path<(ChannelId, MessageId)>,
) -> ApiResult<StatusCode> {
    let channel = db::require_channel(&state.db, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    let (author,): (Uuid,) = sqlx::query_as("SELECT author_id FROM messages WHERE id = $1 AND channel_id = $2")
        .bind(message_id.0)
        .bind(channel_id.0)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    if author != auth.id.0 {
        // In guilds, per channel: overwrites can make someone a moderator of one channel.
        let allowed = channel.guild().is_some()
            && db::channel_permissions_of(&state.db, &channel, auth.id)
                .await?
                .is_some_and(|p| p.allows(Permissions::MANAGE_MESSAGES));
        if !allowed {
            return Err(ApiError::Forbidden);
        }
    }
    sqlx::query("DELETE FROM messages WHERE id = $1").bind(message_id.0).execute(&state.db).await?;
    let audience = db::channel_audience(&state.db, &channel).await?;
    state.events.publish(audience, Event::MessageDelete { channel_id, message_id });
    Ok(StatusCode::NO_CONTENT)
}
