use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use serde::Serialize;
use sivryn_protocol::api::{
    Ban, BanRequest, CreateChannelRequest, CreateGuildRequest, GuildWithChannels, UpdateGuildRequest, UploadedFile,
};
use sivryn_protocol::gateway::{Audience, Event};
use sivryn_protocol::model::{AFK_TIMEOUTS, Channel, ChannelKind, Guild, User};
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{ChannelId, FileId, GuildId, InviteId, RoleId, UserId};
use uuid::Uuid;

use crate::AppState;
use crate::auth::{AuthUser, random_token};
use crate::db::{self, ChannelRow, GuildRow, UserRow};
use crate::error::{ApiError, ApiResult};

fn validate_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if (1..=100).contains(&name.chars().count()) {
        Ok(name.to_owned())
    } else {
        Err(ApiError::BadRequest("name must be 1-100 characters".into()))
    }
}

async fn channels_of(state: &AppState, guild: GuildId) -> ApiResult<Vec<Channel>> {
    let rows: Vec<ChannelRow> = sqlx::query_as("SELECT * FROM channels WHERE guild_id = $1 ORDER BY position, id")
        .bind(guild.0)
        .fetch_all(&state.db)
        .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

async fn my_permissions(state: &AppState, guild: GuildId, user: UserId) -> ApiResult<Permissions> {
    Ok(db::guild_permissions(&state.db, guild, user).await?.unwrap_or(Permissions::empty()))
}

pub async fn list(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Vec<GuildWithChannels>>> {
    let mut out = Vec::new();
    for guild in db::user_guilds(&state.db, auth.id).await? {
        let channels = channels_of(&state, guild.id).await?;
        let permissions = my_permissions(&state, guild.id, auth.id).await?;
        out.push(GuildWithChannels { guild, channels, permissions });
    }
    Ok(Json(out))
}

pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
) -> ApiResult<Json<GuildWithChannels>> {
    db::require(&state.db, guild_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    let guild = db::guild(&state.db, guild_id).await?;
    let channels = channels_of(&state, guild_id).await?;
    let permissions = my_permissions(&state, guild_id, auth.id).await?;
    Ok(Json(GuildWithChannels { guild, channels, permissions }))
}

pub async fn members(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
) -> ApiResult<Json<Vec<User>>> {
    db::require(&state.db, guild_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    let rows: Vec<UserRow> = sqlx::query_as(
        "SELECT u.* FROM users u JOIN guild_members m ON m.user_id = u.id
         WHERE m.guild_id = $1 ORDER BY lower(u.username)",
    )
    .bind(guild_id.0)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// Creates a guild with an `@everyone` role and a default text and voice channel.
pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateGuildRequest>,
) -> ApiResult<(StatusCode, Json<GuildWithChannels>)> {
    let name = validate_name(&req.name)?;
    let guild_id = GuildId::new();
    let mut tx = state.db.begin().await?;

    let guild: GuildRow = sqlx::query_as("INSERT INTO guilds (id, name, owner_id) VALUES ($1, $2, $3) RETURNING *")
        .bind(guild_id.0)
        .bind(&name)
        .bind(auth.id.0)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO roles (id, guild_id, name, permissions, is_everyone) VALUES ($1, $2, '@everyone', $3, TRUE)",
    )
    .bind(RoleId::new().0)
    .bind(guild_id.0)
    .bind(Permissions::default_member().bits() as i64)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO guild_members (guild_id, user_id) VALUES ($1, $2)")
        .bind(guild_id.0)
        .bind(auth.id.0)
        .execute(&mut *tx)
        .await?;

    let mut channels = Vec::new();
    for (position, (name, kind)) in
        [("general", ChannelKind::Text), ("Voice", ChannelKind::Voice)].into_iter().enumerate()
    {
        let row: ChannelRow = sqlx::query_as(
            "INSERT INTO channels (id, guild_id, name, kind, position) VALUES ($1, $2, $3, $4, $5) RETURNING *",
        )
        .bind(ChannelId::new().0)
        .bind(guild_id.0)
        .bind(name)
        .bind(kind.as_str())
        .bind(position as i32)
        .fetch_one(&mut *tx)
        .await?;
        channels.push(Channel::from(row));
    }
    tx.commit().await?;

    let guild: Guild = guild.into();
    state.events.publish(Audience::Users(vec![auth.id]), Event::GuildCreate { guild: guild.clone() });
    Ok((StatusCode::CREATED, Json(GuildWithChannels { guild, channels, permissions: Permissions::all() })))
}

pub async fn create_channel(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
    Json(req): Json<CreateChannelRequest>,
) -> ApiResult<(StatusCode, Json<Channel>)> {
    db::require(&state.db, guild_id, auth.id, Permissions::MANAGE_CHANNELS).await?;
    if req.kind.is_private() {
        return Err(ApiError::BadRequest("guilds have text and voice channels".into()));
    }
    let name = validate_name(&req.name)?;
    let row: ChannelRow = sqlx::query_as(
        "INSERT INTO channels (id, guild_id, name, kind, position, e2ee)
         VALUES ($1, $2, $3, $4, (SELECT COALESCE(max(position) + 1, 0) FROM channels WHERE guild_id = $2), $5)
         RETURNING *",
    )
    .bind(ChannelId::new().0)
    .bind(guild_id.0)
    .bind(name)
    .bind(req.kind.as_str())
    .bind(req.e2ee)
    .fetch_one(&state.db)
    .await?;

    let channel: Channel = row.into();
    state.events.publish(Audience::Guild(guild_id), Event::ChannelCreate { channel: channel.clone() });
    Ok((StatusCode::CREATED, Json(channel)))
}

#[derive(Serialize)]
pub struct GuildInvite {
    pub code: String,
}

pub async fn create_invite(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
) -> ApiResult<Json<GuildInvite>> {
    db::require(&state.db, guild_id, auth.id, Permissions::CREATE_INVITE).await?;
    let code = random_token()[..10].to_owned();
    sqlx::query("INSERT INTO invites (id, code, guild_id, created_by) VALUES ($1, $2, $3, $4)")
        .bind(InviteId::new().0)
        .bind(&code)
        .bind(guild_id.0)
        .bind(auth.id.0)
        .execute(&state.db)
        .await?;
    Ok(Json(GuildInvite { code }))
}

pub async fn accept_invite(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(code): Path<String>,
) -> ApiResult<Json<GuildWithChannels>> {
    let mut tx = state.db.begin().await?;
    let guild_id: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE invites SET uses = uses + 1
         WHERE code = $1 AND guild_id IS NOT NULL
           AND (max_uses IS NULL OR uses < max_uses)
           AND (expires_at IS NULL OR expires_at > now())
         RETURNING guild_id",
    )
    .bind(&code)
    .fetch_optional(&mut *tx)
    .await?;
    let guild_id = GuildId(guild_id.ok_or(ApiError::NotFound)?.0);

    let (banned,): (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM guild_bans WHERE guild_id = $1 AND user_id = $2)")
            .bind(guild_id.0)
            .bind(auth.id.0)
            .fetch_one(&mut *tx)
            .await?;
    if banned {
        return Err(ApiError::Forbidden);
    }

    let inserted = sqlx::query("INSERT INTO guild_members (guild_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(guild_id.0)
        .bind(auth.id.0)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    if inserted.rows_affected() > 0 {
        let user = db::user(&state.db, auth.id).await?;
        state.events.publish(Audience::Guild(guild_id), Event::MemberJoin { guild_id, user: user.into() });
    }

    let guild = db::guild(&state.db, guild_id).await?;
    let channels = channels_of(&state, guild_id).await?;
    let permissions = my_permissions(&state, guild_id, auth.id).await?;
    Ok(Json(GuildWithChannels { guild, channels, permissions }))
}

/// Changes the name or AFK settings (`MANAGE_GUILD`).
pub async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
    Json(req): Json<UpdateGuildRequest>,
) -> ApiResult<Json<Guild>> {
    db::require(&state.db, guild_id, auth.id, Permissions::MANAGE_GUILD).await?;
    let mut tx = state.db.begin().await?;
    if let Some(name) = &req.name {
        sqlx::query("UPDATE guilds SET name = $2 WHERE id = $1")
            .bind(guild_id.0)
            .bind(validate_name(name)?)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(afk) = req.afk_channel_id {
        if let Some(channel) = afk {
            let ok: Option<(Uuid,)> =
                sqlx::query_as("SELECT id FROM channels WHERE id = $1 AND guild_id = $2 AND kind = 'voice'")
                    .bind(channel.0)
                    .bind(guild_id.0)
                    .fetch_optional(&mut *tx)
                    .await?;
            if ok.is_none() {
                return Err(ApiError::BadRequest("AFK channel must be a voice channel of this server".into()));
            }
        }
        sqlx::query("UPDATE guilds SET afk_channel_id = $2 WHERE id = $1")
            .bind(guild_id.0)
            .bind(afk.map(|c| c.0))
            .execute(&mut *tx)
            .await?;
    }
    if let Some(minutes) = req.afk_timeout_minutes {
        if !AFK_TIMEOUTS.contains(&minutes) {
            return Err(ApiError::BadRequest(format!("AFK timeout must be one of {AFK_TIMEOUTS:?} minutes")));
        }
        sqlx::query("UPDATE guilds SET afk_timeout_minutes = $2 WHERE id = $1")
            .bind(guild_id.0)
            .bind(minutes as i32)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let guild = db::guild(&state.db, guild_id).await?;
    state.events.publish(Audience::Guild(guild_id), Event::GuildUpdate { guild: guild.clone() });
    Ok(Json(guild))
}

const MAX_ICON_BYTES: usize = 4 * 1024 * 1024;

/// Sets the guild icon (`MANAGE_GUILD`). Body: a PNG, JPEG, GIF or WebP image.
pub async fn set_icon(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<UploadedFile>> {
    db::require(&state.db, guild_id, auth.id, Permissions::MANAGE_GUILD).await?;
    let content_type = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default();
    if !matches!(content_type, "image/png" | "image/jpeg" | "image/gif" | "image/webp") {
        return Err(ApiError::BadRequest("icon must be a PNG, JPEG, GIF or WebP image".into()));
    }
    if body.is_empty() || body.len() > MAX_ICON_BYTES {
        return Err(ApiError::BadRequest("icon must be at most 4 MB".into()));
    }
    let id = FileId::new();
    crate::storage::save(&state.config.files_dir, id, &body).await.map_err(anyhow::Error::from)?;
    let (old,): (Option<Uuid>,) =
        sqlx::query_as("SELECT icon_id FROM guilds WHERE id = $1").bind(guild_id.0).fetch_one(&state.db).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("INSERT INTO files (id, uploader_id, guild_id, size, content_type) VALUES ($1, $2, $3, $4, $5)")
        .bind(id.0)
        .bind(auth.id.0)
        .bind(guild_id.0)
        .bind(body.len() as i64)
        .bind(content_type)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE guilds SET icon_id = $2 WHERE id = $1").bind(guild_id.0).bind(id.0).execute(&mut *tx).await?;
    if let Some(old) = old {
        sqlx::query("DELETE FROM files WHERE id = $1").bind(old).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    if let Some(old) = old {
        crate::storage::remove(&state.config.files_dir, FileId(old)).await;
    }

    let guild = db::guild(&state.db, guild_id).await?;
    state.events.publish(Audience::Guild(guild_id), Event::GuildUpdate { guild });
    Ok(Json(UploadedFile { id, size: body.len() as u64 }))
}

/// Removes a member: yourself (leave) or someone else (`KICK_MEMBERS`).
pub async fn remove_member(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((guild_id, user_id)): Path<(GuildId, UserId)>,
) -> ApiResult<StatusCode> {
    let required = if user_id == auth.id { Permissions::empty() } else { Permissions::KICK_MEMBERS };
    db::require(&state.db, guild_id, auth.id, required).await?;
    let guild = db::guild(&state.db, guild_id).await?;
    if user_id == guild.owner_id {
        // The owner can neither be kicked nor leave their own server.
        return Err(ApiError::Forbidden);
    }
    let removed = sqlx::query("DELETE FROM guild_members WHERE guild_id = $1 AND user_id = $2")
        .bind(guild_id.0)
        .bind(user_id.0)
        .execute(&state.db)
        .await?;
    if removed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    after_member_removed(&state, guild_id, user_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Tells the guild (and the removed user) and drops the user from its voice channels.
async fn after_member_removed(state: &AppState, guild_id: GuildId, user_id: UserId) {
    state.events.publish(Audience::Guild(guild_id), Event::MemberLeave { guild_id, user_id });
    let voice: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM channels WHERE guild_id = $1 AND kind = 'voice'")
        .bind(guild_id.0)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    for (channel,) in voice {
        crate::routes::voice::remove_participant(state, ChannelId(channel), user_id).await;
    }
}

pub async fn bans(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
) -> ApiResult<Json<Vec<Ban>>> {
    db::require(&state.db, guild_id, auth.id, Permissions::BAN_MEMBERS).await?;
    let rows: Vec<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT user_id, reason FROM guild_bans WHERE guild_id = $1 ORDER BY created_at")
            .bind(guild_id.0)
            .fetch_all(&state.db)
            .await?;
    let mut out = Vec::new();
    for (user, reason) in rows {
        out.push(Ban { user: db::user(&state.db, UserId(user)).await?.into(), reason });
    }
    Ok(Json(out))
}

pub async fn ban(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((guild_id, user_id)): Path<(GuildId, UserId)>,
    body: Option<Json<BanRequest>>,
) -> ApiResult<StatusCode> {
    db::require(&state.db, guild_id, auth.id, Permissions::BAN_MEMBERS).await?;
    let guild = db::guild(&state.db, guild_id).await?;
    if user_id == guild.owner_id || user_id == auth.id {
        return Err(ApiError::Forbidden);
    }
    db::user(&state.db, user_id).await?;
    let reason = body.and_then(|b| b.0.reason).map(|r| r.trim().chars().take(500).collect::<String>());
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO guild_bans (guild_id, user_id, banned_by, reason) VALUES ($1, $2, $3, $4)
         ON CONFLICT (guild_id, user_id) DO UPDATE SET reason = excluded.reason",
    )
    .bind(guild_id.0)
    .bind(user_id.0)
    .bind(auth.id.0)
    .bind(reason)
    .execute(&mut *tx)
    .await?;
    let removed = sqlx::query("DELETE FROM guild_members WHERE guild_id = $1 AND user_id = $2")
        .bind(guild_id.0)
        .bind(user_id.0)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if removed.rows_affected() > 0 {
        after_member_removed(&state, guild_id, user_id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unban(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((guild_id, user_id)): Path<(GuildId, UserId)>,
) -> ApiResult<StatusCode> {
    db::require(&state.db, guild_id, auth.id, Permissions::BAN_MEMBERS).await?;
    let removed = sqlx::query("DELETE FROM guild_bans WHERE guild_id = $1 AND user_id = $2")
        .bind(guild_id.0)
        .bind(user_id.0)
        .execute(&state.db)
        .await?;
    if removed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Everyone who can read a channel: guild members, or the DM's members.
/// Clients use it to build the channel's MLS group.
pub async fn channel_members(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
) -> ApiResult<Json<Vec<User>>> {
    let channel = db::require_channel(&state.db, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    let ids: Vec<UserId> = match channel.guild() {
        Some(guild) => sqlx::query_as::<_, (Uuid,)>("SELECT user_id FROM guild_members WHERE guild_id = $1")
            .bind(guild.0)
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .map(|(id,)| UserId(id))
            .collect(),
        None => db::channel_member_ids(&state.db, channel_id).await?,
    };
    Ok(Json(db::users(&state.db, &ids).await?))
}
