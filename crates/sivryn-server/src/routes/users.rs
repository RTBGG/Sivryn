//! User profiles, status, presence, read states, mutes and per-user settings.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use chrono::{DateTime, Utc};
use sivryn_protocol::api::{
    AckRequest, ChannelMute, MeSettings, MuteRequest, ReadState, Relationship, UpdateMeRequest, UploadedFile,
    UserPresence, UserProfile,
};
use sivryn_protocol::gateway::{Audience, Event};
use sivryn_protocol::model::UserStatus;
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{ChannelId, FileId, GuildId, MessageId, UserId};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::db::{self, UserRow};
use crate::error::{ApiError, ApiResult};

/// Profile pictures and banners can be at most this large.
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_BIO_CHARS: usize = 190;
const MAX_NAME_CHARS: usize = 32;

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

/// What others see: the status while connected, nothing while offline or invisible.
pub fn visible_status(state: &AppState, user: &UserRow) -> Option<UserStatus> {
    let status = user.status();
    (state.presence.is_online(UserId(user.id)) && status != UserStatus::Invisible).then_some(status)
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
    let me = user_id == auth.id;
    let related =
        me || relationship != Relationship::None || db::related_users(&state.db, auth.id).await?.contains(&user_id);
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
    // You always see your own status, even while invisible.
    let status = if me { Some(user.status()) } else { visible_status(&state, &user) };
    Ok(Json(UserProfile {
        relationship,
        mutual_guilds: mutual.into_iter().map(|(g,)| GuildId(g)).collect(),
        bio: user.bio.clone(),
        banner_id: user.banner_id.map(FileId),
        status,
        user: user.into(),
    }))
}

/// Status of the users you can see who are online right now (and your own).
pub async fn presence(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Vec<UserPresence>>> {
    let mut ids = db::related_users(&state.db, auth.id).await?;
    ids.retain(|u| state.presence.is_online(*u));
    let ids: Vec<Uuid> = ids.into_iter().map(|u| u.0).collect();
    let rows: Vec<UserRow> =
        sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(&ids).fetch_all(&state.db).await?;
    let mut out: Vec<UserPresence> = rows
        .iter()
        .filter_map(|u| visible_status(&state, u).map(|status| UserPresence { user_id: UserId(u.id), status }))
        .collect();
    let me = db::user(&state.db, auth.id).await?;
    out.push(UserPresence { user_id: auth.id, status: me.status() });
    Ok(Json(out))
}

/// Tells friends and people sharing a guild or DM what `user` looks like now
/// (online with a status, or offline), and the user's own other devices.
pub async fn announce_presence(state: &AppState, user_id: UserId) {
    let result = async {
        let user = db::user(&state.db, user_id).await?;
        let others = db::related_users(&state.db, user_id).await?;
        if !others.is_empty() {
            let status = visible_status(state, &user);
            state.events.publish(Audience::Users(others), Event::PresenceUpdate { user_id, status });
        }
        let own = state.presence.is_online(user_id).then(|| user.status());
        state.events.publish(Audience::Users(vec![user_id]), Event::PresenceUpdate { user_id, status: own });
        ApiResult::Ok(())
    }
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "could not announce presence");
    }
}

/// Sends the user's new name or picture to everyone who can see them.
async fn announce_user(state: &AppState, user_id: UserId) -> ApiResult<()> {
    let user = db::user(&state.db, user_id).await?;
    let mut to = db::related_users(&state.db, user_id).await?;
    to.push(user_id);
    state.events.publish(Audience::Users(to), Event::UserUpdate { user: user.into() });
    Ok(())
}

pub async fn settings(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<MeSettings>> {
    let user = db::user(&state.db, auth.id).await?;
    Ok(Json(MeSettings {
        allow_server_dms: user.allow_server_dms,
        status: user.status(),
        bio: user.bio,
        banner_id: user.banner_id.map(FileId),
    }))
}

pub async fn update_me(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<UpdateMeRequest>,
) -> ApiResult<Json<MeSettings>> {
    let mut tx = state.db.begin().await?;
    if let Some(allow) = req.allow_server_dms {
        sqlx::query("UPDATE users SET allow_server_dms = $2 WHERE id = $1")
            .bind(auth.id.0)
            .bind(allow)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(name) = &req.display_name {
        let name = name.as_deref().map(str::trim).filter(|n| !n.is_empty());
        if name.is_some_and(|n| n.chars().count() > MAX_NAME_CHARS) {
            return Err(ApiError::BadRequest(format!("names can be at most {MAX_NAME_CHARS} characters")));
        }
        sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
            .bind(auth.id.0)
            .bind(name)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(bio) = &req.bio {
        let bio = bio.trim();
        if bio.chars().count() > MAX_BIO_CHARS {
            return Err(ApiError::BadRequest(format!("the description can be at most {MAX_BIO_CHARS} characters")));
        }
        sqlx::query("UPDATE users SET bio = $2 WHERE id = $1").bind(auth.id.0).bind(bio).execute(&mut *tx).await?;
    }
    if let Some(status) = req.status {
        sqlx::query("UPDATE users SET status = $2 WHERE id = $1")
            .bind(auth.id.0)
            .bind(status.as_str())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    if req.display_name.is_some() {
        announce_user(&state, auth.id).await?;
    }
    if req.status.is_some() {
        announce_presence(&state, auth.id).await;
    }
    settings(State(state), auth).await
}

#[derive(Clone, Copy)]
pub enum ProfileImage {
    Avatar,
    Banner,
}

impl ProfileImage {
    fn column(self) -> &'static str {
        match self {
            Self::Avatar => "avatar_id",
            Self::Banner => "banner_id",
        }
    }
}

/// Replaces (or with `None`, removes) the avatar or banner.
async fn replace_image(
    state: &AppState,
    user: UserId,
    kind: ProfileImage,
    image: Option<(&str, &[u8])>,
) -> ApiResult<Option<FileId>> {
    let column = kind.column();
    let (old,): (Option<Uuid>,) =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {column} FROM users WHERE id = $1")))
            .bind(user.0)
            .fetch_one(&state.db)
            .await?;
    let new = match image {
        Some((content_type, body)) => {
            let id = FileId::new();
            crate::storage::save(&state.config.files_dir, id, body).await.map_err(anyhow::Error::from)?;
            Some((id, content_type, body.len()))
        }
        None => None,
    };
    let mut tx = state.db.begin().await?;
    if let Some((id, content_type, size)) = new {
        sqlx::query(
            "INSERT INTO files (id, uploader_id, owner_user_id, size, content_type) VALUES ($1, $2, $2, $3, $4)",
        )
        .bind(id.0)
        .bind(user.0)
        .bind(size as i64)
        .bind(content_type)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE users SET {column} = $2 WHERE id = $1")))
        .bind(user.0)
        .bind(new.map(|(id, _, _)| id.0))
        .execute(&mut *tx)
        .await?;
    if let Some(old) = old {
        sqlx::query("DELETE FROM files WHERE id = $1").bind(old).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    if let Some(old) = old {
        crate::storage::remove(&state.config.files_dir, FileId(old)).await;
    }
    Ok(new.map(|(id, _, _)| id))
}

fn image_upload<'a>(headers: &'a HeaderMap, body: &'a Bytes) -> ApiResult<(&'a str, &'a [u8])> {
    let content_type = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default();
    if !matches!(content_type, "image/png" | "image/jpeg" | "image/gif" | "image/webp") {
        return Err(ApiError::BadRequest("images must be PNG, JPEG, GIF or WebP".into()));
    }
    if body.is_empty() || body.len() > MAX_IMAGE_BYTES {
        return Err(ApiError::BadRequest("images can be at most 4 MB".into()));
    }
    Ok((content_type, body))
}

/// `PUT /users/@me/avatar` and `/banner`. Body: the image.
pub async fn set_image(
    state: AppState,
    auth: AuthUser,
    kind: ProfileImage,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<UploadedFile>> {
    let image = image_upload(&headers, &body)?;
    let id = replace_image(&state, auth.id, kind, Some(image)).await?.expect("an image was stored");
    announce_user(&state, auth.id).await?;
    Ok(Json(UploadedFile { id, size: body.len() as u64 }))
}

pub async fn set_avatar(
    State(state): State<AppState>,
    auth: AuthUser,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<UploadedFile>> {
    set_image(state, auth, ProfileImage::Avatar, headers, body).await
}

pub async fn set_banner(
    State(state): State<AppState>,
    auth: AuthUser,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<UploadedFile>> {
    set_image(state, auth, ProfileImage::Banner, headers, body).await
}

pub async fn remove_avatar(State(state): State<AppState>, auth: AuthUser) -> ApiResult<StatusCode> {
    replace_image(&state, auth.id, ProfileImage::Avatar, None).await?;
    announce_user(&state, auth.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_banner(State(state): State<AppState>, auth: AuthUser) -> ApiResult<StatusCode> {
    replace_image(&state, auth.id, ProfileImage::Banner, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Unread counts for every channel the user can read. Without a read state,
/// messages since joining count.
pub async fn read_states(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Vec<ReadState>>> {
    let rows: Vec<(Uuid, Option<Uuid>, i32)> = sqlx::query_as(
        "WITH visible AS (
             SELECT c.id, m.joined_at AS since FROM channels c
             JOIN guild_members m ON m.guild_id = c.guild_id AND m.user_id = $1
             UNION ALL
             SELECT c.id, cm.joined_at FROM channels c
             JOIN channel_members cm ON cm.channel_id = c.id AND cm.user_id = $1
         )
         SELECT v.id, rs.last_read_id, (
             SELECT count(*) FROM (
                 SELECT 1 FROM messages m
                 WHERE m.channel_id = v.id AND m.kind = 'message' AND m.author_id <> $1
                   AND CASE WHEN rs.last_read_id IS NULL THEN m.created_at > v.since ELSE m.id > rs.last_read_id END
                 LIMIT 100
             ) n
         )::INT
         FROM visible v
         LEFT JOIN read_states rs ON rs.channel_id = v.id AND rs.user_id = $1",
    )
    .bind(auth.id.0)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(channel, last, unread)| ReadState {
                channel_id: ChannelId(channel),
                last_read_id: last.map(MessageId),
                unread: unread.max(0) as u32,
            })
            .collect(),
    ))
}

/// `PUT /channels/{id}/read`: marks the channel read up to a message.
pub async fn ack(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
    Json(req): Json<AckRequest>,
) -> ApiResult<StatusCode> {
    db::require_channel(&state.db, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    let in_channel: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM messages WHERE id = $1 AND channel_id = $2")
        .bind(req.message_id.0)
        .bind(channel_id.0)
        .fetch_optional(&state.db)
        .await?;
    if in_channel.is_none() {
        return Err(ApiError::NotFound);
    }
    sqlx::query(
        "INSERT INTO read_states (user_id, channel_id, last_read_id) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, channel_id)
         DO UPDATE SET last_read_id = GREATEST(read_states.last_read_id, excluded.last_read_id)",
    )
    .bind(auth.id.0)
    .bind(channel_id.0)
    .bind(req.message_id.0)
    .execute(&state.db)
    .await?;
    state
        .events
        .publish(Audience::Users(vec![auth.id]), Event::ReadStateUpdate { channel_id, last_read_id: req.message_id });
    Ok(StatusCode::NO_CONTENT)
}

pub async fn mutes(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Vec<ChannelMute>>> {
    let rows: Vec<(Uuid, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT channel_id, until FROM channel_mutes WHERE user_id = $1 AND (until IS NULL OR until > now())",
    )
    .bind(auth.id.0)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.into_iter().map(|(c, until)| ChannelMute { channel_id: ChannelId(c), until }).collect()))
}

/// `PUT /channels/{id}/mute`: mute until a time, or until unmuted.
pub async fn mute(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
    body: Option<Json<MuteRequest>>,
) -> ApiResult<StatusCode> {
    db::require_channel(&state.db, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    let until = body.and_then(|b| b.0.until);
    sqlx::query(
        "INSERT INTO channel_mutes (user_id, channel_id, until) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, channel_id) DO UPDATE SET until = excluded.until",
    )
    .bind(auth.id.0)
    .bind(channel_id.0)
    .bind(until)
    .execute(&state.db)
    .await?;
    state.events.publish(Audience::Users(vec![auth.id]), Event::NotificationSettingsChanged);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unmute(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM channel_mutes WHERE user_id = $1 AND channel_id = $2")
        .bind(auth.id.0)
        .bind(channel_id.0)
        .execute(&state.db)
        .await?;
    state.events.publish(Audience::Users(vec![auth.id]), Event::NotificationSettingsChanged);
    Ok(StatusCode::NO_CONTENT)
}
