//! Encrypted attachments and guild icons.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use sivryn_protocol::api::UploadedFile;
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{ChannelId, FileId, GuildId};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::db;
use crate::error::{ApiError, ApiResult};

/// Stores an attachment's ciphertext for a channel (`ATTACH_FILES`).
pub async fn upload(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<UploadedFile>)> {
    db::require_channel(&state.db, channel_id, auth.id, Permissions::ATTACH_FILES).await?;
    if body.is_empty() {
        return Err(ApiError::BadRequest("empty file".into()));
    }
    if body.len() as u64 > state.config.max_upload_bytes {
        return Err(ApiError::BadRequest(format!(
            "files can be at most {} MB",
            state.config.max_upload_bytes / 1024 / 1024
        )));
    }
    let id = FileId::new();
    crate::storage::save(&state.config.files_dir, id, &body).await.map_err(anyhow::Error::from)?;
    sqlx::query("INSERT INTO files (id, uploader_id, channel_id, size) VALUES ($1, $2, $3, $4)")
        .bind(id.0)
        .bind(auth.id.0)
        .bind(channel_id.0)
        .bind(body.len() as i64)
        .execute(&state.db)
        .await?;
    Ok((StatusCode::CREATED, Json(UploadedFile { id, size: body.len() as u64 })))
}

/// Serves a file to anyone who may read its channel (attachments) or who is
/// a member of its guild (icons).
pub async fn download(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(file_id): Path<FileId>,
) -> ApiResult<impl IntoResponse> {
    let row: Option<(Option<Uuid>, Option<Uuid>, String)> =
        sqlx::query_as("SELECT channel_id, guild_id, content_type FROM files WHERE id = $1")
            .bind(file_id.0)
            .fetch_optional(&state.db)
            .await?;
    let (channel, guild, content_type) = row.ok_or(ApiError::NotFound)?;
    match (channel, guild) {
        (Some(channel), _) => {
            db::require_channel(&state.db, ChannelId(channel), auth.id, Permissions::VIEW_CHANNEL).await?;
        }
        (None, Some(guild)) => db::require(&state.db, GuildId(guild), auth.id, Permissions::empty()).await?,
        (None, None) => return Err(ApiError::NotFound),
    }
    let bytes = crate::storage::load(&state.config.files_dir, file_id).await.map_err(|_| ApiError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            // Content never changes for an ID.
            (header::CACHE_CONTROL, "private, max-age=31536000, immutable".to_owned()),
        ],
        bytes,
    ))
}
