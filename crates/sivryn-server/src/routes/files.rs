//! Encrypted attachments and guild icons.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
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

/// A file's channel, guild, owner and content type.
type FileAccessRow = (Option<Uuid>, Option<Uuid>, Option<Uuid>, String);

/// The start of an open-ended `Range: bytes=N-` request (the form clients
/// use to resume a download). Other forms get the whole file.
fn range_start(headers: &HeaderMap) -> Option<u64> {
    let value = headers.get(header::RANGE)?.to_str().ok()?;
    let rest = value.trim().strip_prefix("bytes=")?;
    let (start, end) = rest.split_once('-')?;
    if !end.trim().is_empty() {
        return None;
    }
    start.trim().parse().ok()
}

/// Serves a file to anyone who may read its channel (attachments), who is a
/// member of its guild (icons), or to anyone signed in (avatars, banners).
/// `Range: bytes=N-` resumes an interrupted download.
pub async fn download(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(file_id): Path<FileId>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let row: Option<FileAccessRow> =
        sqlx::query_as("SELECT channel_id, guild_id, owner_user_id, content_type FROM files WHERE id = $1")
            .bind(file_id.0)
            .fetch_optional(&state.db)
            .await?;
    let (channel, guild, owner, content_type) = row.ok_or(ApiError::NotFound)?;
    match (channel, guild, owner) {
        (Some(channel), _, _) => {
            db::require_channel(&state.db, ChannelId(channel), auth.id, Permissions::VIEW_CHANNEL).await?;
        }
        (None, Some(guild), _) => db::require(&state.db, GuildId(guild), auth.id, Permissions::empty()).await?,
        // Avatars and banners are public to signed-in users.
        (None, None, Some(_)) => {}
        (None, None, None) => return Err(ApiError::NotFound),
    }
    let bytes = crate::storage::load(&state.config.files_dir, file_id).await.map_err(|_| ApiError::NotFound)?;
    let total = bytes.len() as u64;
    let common = [
        (header::CONTENT_TYPE, content_type),
        // Content never changes for an ID.
        (header::CACHE_CONTROL, "private, max-age=31536000, immutable".to_owned()),
        (header::ACCEPT_RANGES, "bytes".to_owned()),
    ];
    match range_start(&headers) {
        Some(start) if start >= total => {
            Ok((StatusCode::RANGE_NOT_SATISFIABLE, [(header::CONTENT_RANGE, format!("bytes */{total}"))])
                .into_response())
        }
        Some(start) => {
            let part = Bytes::from(bytes).slice(start as usize..);
            let range = format!("bytes {start}-{}/{total}", total - 1);
            Ok((StatusCode::PARTIAL_CONTENT, common, [(header::CONTENT_RANGE, range)], part).into_response())
        }
        None => Ok((common, bytes).into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_open_ended_ranges_resume() {
        let with = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::RANGE, v.parse().unwrap());
            range_start(&h)
        };
        assert_eq!(with("bytes=1024-"), Some(1024));
        assert_eq!(with("bytes=0-"), Some(0));
        assert_eq!(with("bytes=0-99"), None);
        assert_eq!(with("bytes=-500"), None);
        assert_eq!(with("items=5-"), None);
        assert_eq!(range_start(&HeaderMap::new()), None);
    }
}
