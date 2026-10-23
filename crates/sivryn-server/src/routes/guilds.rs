use std::collections::HashMap;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use serde::Serialize;
use sivryn_protocol::api::{
    Ban, BanRequest, CategoryRequest, CreateChannelRequest, CreateGuildRequest, GuildMember, GuildWithChannels,
    UpdateChannelRequest, UpdateGuildRequest, UpdateMemberRequest, UploadedFile,
};
use sivryn_protocol::gateway::{Audience, Event};
use sivryn_protocol::model::{AFK_TIMEOUTS, AUDIO_BITRATES, Category, Channel, ChannelKind, Guild, NotifyLevel, User};
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{CategoryId, ChannelId, FileId, GuildId, InviteId, RoleId, UserId};
use uuid::Uuid;

use crate::AppState;
use crate::auth::{AuthUser, random_token};
use crate::db::{self, CategoryRow, ChannelRow, GuildRow};
use crate::error::{ApiError, ApiResult};

fn validate_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if (1..=100).contains(&name.chars().count()) {
        Ok(name.to_owned())
    } else {
        Err(ApiError::BadRequest("name must be 1-100 characters".into()))
    }
}

/// A guild as one member sees it: the channels and categories they can see
/// (with their overwrites), roles, their permissions and their own settings.
async fn guild_for(state: &AppState, guild: Guild, user: UserId) -> ApiResult<GuildWithChannels> {
    let (nick, level): (Option<String>, String) =
        sqlx::query_as("SELECT nick, notify_level FROM guild_members WHERE guild_id = $1 AND user_id = $2")
            .bind(guild.id.0)
            .bind(user.0)
            .fetch_optional(&state.db)
            .await?
            .unwrap_or((None, "all".into()));
    let member = db::membership(&state.db, guild.id, user).await?;
    let overwrites = db::guild_overwrites(&state.db, guild.id).await?;
    let rows: Vec<ChannelRow> = sqlx::query_as("SELECT * FROM channels WHERE guild_id = $1 ORDER BY position, id")
        .bind(guild.id.0)
        .fetch_all(&state.db)
        .await?;
    let mut channel_permissions = HashMap::new();
    let mut channels = Vec::new();
    for row in rows {
        let permissions = member.as_ref().map_or(Permissions::empty(), |m| m.in_channel(user, &overwrites, &row));
        if !permissions.allows(Permissions::VIEW_CHANNEL) {
            continue;
        }
        let mut channel: Channel = row.into();
        channel.overwrites = overwrites.channels.get(&channel.id).cloned().unwrap_or_default();
        channel_permissions.insert(channel.id, permissions);
        channels.push(channel);
    }
    let category_rows: Vec<CategoryRow> =
        sqlx::query_as("SELECT * FROM categories WHERE guild_id = $1 ORDER BY position, created_at")
            .bind(guild.id.0)
            .fetch_all(&state.db)
            .await?;
    // A category shows up if it's visible itself or holds a visible channel.
    let categories = category_rows
        .into_iter()
        .map(Category::from)
        .filter(|c| {
            member.as_ref().is_some_and(|m| m.in_category(user, &overwrites, c.id).allows(Permissions::VIEW_CHANNEL))
                || channels.iter().any(|ch| ch.category_id == Some(c.id))
        })
        .map(|mut c| {
            c.overwrites = overwrites.categories.get(&c.id).cloned().unwrap_or_default();
            c
        })
        .collect();
    Ok(GuildWithChannels {
        channels,
        categories,
        permissions: member.as_ref().map_or(Permissions::empty(), |m| m.permissions),
        nick,
        notify_level: NotifyLevel::parse(&level),
        roles: db::roles(&state.db, guild.id).await?,
        channel_permissions,
        guild,
    })
}

pub async fn list(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Vec<GuildWithChannels>>> {
    let mut out = Vec::new();
    for guild in db::user_guilds(&state.db, auth.id).await? {
        out.push(guild_for(&state, guild, auth.id).await?);
    }
    Ok(Json(out))
}

pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
) -> ApiResult<Json<GuildWithChannels>> {
    db::require(&state.db, guild_id, auth.id, Permissions::empty()).await?;
    let guild = db::guild(&state.db, guild_id).await?;
    Ok(Json(guild_for(&state, guild, auth.id).await?))
}

pub async fn members(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
) -> ApiResult<Json<Vec<GuildMember>>> {
    db::require(&state.db, guild_id, auth.id, Permissions::empty()).await?;
    let ids: Vec<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT user_id, nick FROM guild_members WHERE guild_id = $1")
            .bind(guild_id.0)
            .fetch_all(&state.db)
            .await?;
    let memberships = db::memberships(&state.db, guild_id).await?;
    let users: Vec<UserId> = ids.iter().map(|(id, _)| UserId(*id)).collect();
    let mut out: Vec<GuildMember> = db::users(&state.db, &users)
        .await?
        .into_iter()
        .map(|user| {
            let nick = ids.iter().find(|(id, _)| *id == user.id.0).and_then(|(_, n)| n.clone());
            let roles =
                memberships.iter().find(|(id, _)| *id == user.id).map(|(_, m)| m.roles.clone()).unwrap_or_default();
            GuildMember { user, nick, roles }
        })
        .collect();
    out.sort_by_key(|m| m.nick.clone().unwrap_or_else(|| m.user.name().to_owned()).to_lowercase());
    Ok(Json(out))
}

/// `PATCH /guilds/{id}/members/@me`: your name and notifications in a guild.
pub async fn update_me_member(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
    Json(req): Json<UpdateMemberRequest>,
) -> ApiResult<StatusCode> {
    db::require(&state.db, guild_id, auth.id, Permissions::empty()).await?;
    if let Some(nick) = &req.nick {
        let nick = nick.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
        if nick.as_ref().is_some_and(|n| n.chars().count() > 32) {
            return Err(ApiError::BadRequest("names can be at most 32 characters".into()));
        }
        sqlx::query("UPDATE guild_members SET nick = $3 WHERE guild_id = $1 AND user_id = $2")
            .bind(guild_id.0)
            .bind(auth.id.0)
            .bind(&nick)
            .execute(&state.db)
            .await?;
        state.events.publish(Audience::Guild(guild_id), Event::MemberUpdate { guild_id, user_id: auth.id, nick });
    }
    if let Some(level) = req.notify_level {
        sqlx::query("UPDATE guild_members SET notify_level = $3 WHERE guild_id = $1 AND user_id = $2")
            .bind(guild_id.0)
            .bind(auth.id.0)
            .bind(level.as_str())
            .execute(&state.db)
            .await?;
        state.events.publish(Audience::Users(vec![auth.id]), Event::NotificationSettingsChanged);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Deletes the guild with everything in it (owner only).
pub async fn delete(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
) -> ApiResult<StatusCode> {
    let guild = db::guild(&state.db, guild_id).await.map_err(|_| ApiError::NotFound)?;
    if guild.owner_id != auth.id {
        return Err(ApiError::Forbidden);
    }
    let files: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT f.id FROM files f LEFT JOIN channels c ON c.id = f.channel_id
         WHERE f.guild_id = $1 OR c.guild_id = $1",
    )
    .bind(guild_id.0)
    .fetch_all(&state.db)
    .await?;
    // Members leave voice before the rooms disappear.
    let voice: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM channels WHERE guild_id = $1 AND kind = 'voice'")
        .bind(guild_id.0)
        .fetch_all(&state.db)
        .await?;
    // The event goes out first: once the guild is gone, nobody is a member.
    state.events.publish(Audience::Guild(guild_id), Event::GuildDelete { guild_id });
    sqlx::query("DELETE FROM guilds WHERE id = $1").bind(guild_id.0).execute(&state.db).await?;
    for (id,) in files {
        crate::storage::remove(&state.config.files_dir, FileId(id)).await;
    }
    for (channel,) in voice {
        crate::routes::voice::close_room(&state, ChannelId(channel)).await;
    }
    Ok(StatusCode::NO_CONTENT)
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

    let text_category = req.text_category.as_deref().map(validate_name).transpose()?.unwrap_or("Text Channels".into());
    let voice_category =
        req.voice_category.as_deref().map(validate_name).transpose()?.unwrap_or("Voice Channels".into());
    for (position, (name, kind)) in
        [(text_category, ChannelKind::Text), (voice_category, ChannelKind::Voice)].into_iter().enumerate()
    {
        let category = CategoryId::new();
        sqlx::query("INSERT INTO categories (id, guild_id, name, position) VALUES ($1, $2, $3, $4)")
            .bind(category.0)
            .bind(guild_id.0)
            .bind(&name)
            .bind(position as i32)
            .execute(&mut *tx)
            .await?;
        let channel_name = if kind == ChannelKind::Text { "general" } else { "Voice" };
        sqlx::query(
            "INSERT INTO channels (id, guild_id, name, kind, position, category_id) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(ChannelId::new().0)
        .bind(guild_id.0)
        .bind(channel_name)
        .bind(kind.as_str())
        .bind(position as i32)
        .bind(category.0)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let guild: Guild = guild.into();
    state.events.publish(Audience::Users(vec![auth.id]), Event::GuildCreate { guild: guild.clone() });
    Ok((StatusCode::CREATED, Json(guild_for(&state, guild, auth.id).await?)))
}

async fn require_category(state: &AppState, guild: GuildId, category: CategoryId) -> ApiResult<()> {
    let found: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM categories WHERE id = $1 AND guild_id = $2")
        .bind(category.0)
        .bind(guild.0)
        .fetch_optional(&state.db)
        .await?;
    found.map(|_| ()).ok_or_else(|| ApiError::BadRequest("unknown category".into()))
}

/// Tells members to reload the guild after its channel list changed.
async fn publish_guild_update(state: &AppState, guild_id: GuildId) -> ApiResult<()> {
    let guild = db::guild(&state.db, guild_id).await?;
    state.events.publish(Audience::Guild(guild_id), Event::GuildUpdate { guild });
    Ok(())
}

pub async fn create_category(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
    Json(req): Json<CategoryRequest>,
) -> ApiResult<(StatusCode, Json<Category>)> {
    db::require(&state.db, guild_id, auth.id, Permissions::MANAGE_CHANNELS).await?;
    let row: CategoryRow = sqlx::query_as(
        "INSERT INTO categories (id, guild_id, name, position)
         VALUES ($1, $2, $3, (SELECT COALESCE(max(position) + 1, 0) FROM categories WHERE guild_id = $2))
         RETURNING id, guild_id, name, position",
    )
    .bind(CategoryId::new().0)
    .bind(guild_id.0)
    .bind(validate_name(&req.name)?)
    .fetch_one(&state.db)
    .await?;
    publish_guild_update(&state, guild_id).await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

async fn category_guild(state: &AppState, category: CategoryId) -> ApiResult<GuildId> {
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT guild_id FROM categories WHERE id = $1")
        .bind(category.0)
        .fetch_optional(&state.db)
        .await?;
    row.map(|(g,)| GuildId(g)).ok_or(ApiError::NotFound)
}

pub async fn update_category(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(category_id): Path<CategoryId>,
    Json(req): Json<CategoryRequest>,
) -> ApiResult<StatusCode> {
    let guild_id = category_guild(&state, category_id).await?;
    db::require(&state.db, guild_id, auth.id, Permissions::MANAGE_CHANNELS).await?;
    sqlx::query("UPDATE categories SET name = $2 WHERE id = $1")
        .bind(category_id.0)
        .bind(validate_name(&req.name)?)
        .execute(&state.db)
        .await?;
    publish_guild_update(&state, guild_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Deletes a category; its channels stay, without a category.
pub async fn delete_category(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(category_id): Path<CategoryId>,
) -> ApiResult<StatusCode> {
    let guild_id = category_guild(&state, category_id).await?;
    db::require(&state.db, guild_id, auth.id, Permissions::MANAGE_CHANNELS).await?;
    sqlx::query("DELETE FROM categories WHERE id = $1").bind(category_id.0).execute(&state.db).await?;
    publish_guild_update(&state, guild_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Renames a channel or moves it to another category (`MANAGE_CHANNELS`).
pub async fn update_channel(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
    Json(req): Json<UpdateChannelRequest>,
) -> ApiResult<Json<Channel>> {
    let channel = db::channel(&state.db, channel_id).await.map_err(|_| ApiError::NotFound)?;
    let guild_id = channel.guild().ok_or_else(|| ApiError::BadRequest("only guild channels can be edited".into()))?;
    db::require(&state.db, guild_id, auth.id, Permissions::MANAGE_CHANNELS).await?;
    if let Some(name) = &req.name {
        sqlx::query("UPDATE channels SET name = $2 WHERE id = $1")
            .bind(channel_id.0)
            .bind(validate_name(name)?)
            .execute(&state.db)
            .await?;
    }
    if let Some(category) = req.category_id {
        if let Some(category) = category {
            require_category(&state, guild_id, category).await?;
        }
        sqlx::query("UPDATE channels SET category_id = $2 WHERE id = $1")
            .bind(channel_id.0)
            .bind(category.map(|c| c.0))
            .execute(&state.db)
            .await?;
    }
    publish_guild_update(&state, guild_id).await?;
    Ok(Json(db::channel(&state.db, channel_id).await?.into()))
}

/// Deletes a channel and its messages (`MANAGE_CHANNELS`).
pub async fn delete_channel(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
) -> ApiResult<StatusCode> {
    let channel = db::channel(&state.db, channel_id).await.map_err(|_| ApiError::NotFound)?;
    let guild_id = channel.guild().ok_or_else(|| ApiError::BadRequest("only guild channels can be deleted".into()))?;
    db::require(&state.db, guild_id, auth.id, Permissions::MANAGE_CHANNELS).await?;
    let files: Vec<(Uuid,)> =
        sqlx::query_as("SELECT id FROM files WHERE channel_id = $1").bind(channel_id.0).fetch_all(&state.db).await?;
    sqlx::query("DELETE FROM channels WHERE id = $1").bind(channel_id.0).execute(&state.db).await?;
    for (id,) in files {
        crate::storage::remove(&state.config.files_dir, FileId(id)).await;
    }
    if channel.kind() == ChannelKind::Voice {
        crate::routes::voice::close_room(&state, channel_id).await;
    }
    state.events.publish(Audience::Guild(guild_id), Event::ChannelDelete { guild_id: Some(guild_id), channel_id });
    Ok(StatusCode::NO_CONTENT)
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
    if let Some(category) = req.category_id {
        require_category(&state, guild_id, category).await?;
    }
    let row: ChannelRow = sqlx::query_as(
        "INSERT INTO channels (id, guild_id, name, kind, position, e2ee, category_id)
         VALUES ($1, $2, $3, $4, (SELECT COALESCE(max(position) + 1, 0) FROM channels WHERE guild_id = $2), $5, $6)
         RETURNING *",
    )
    .bind(ChannelId::new().0)
    .bind(guild_id.0)
    .bind(name)
    .bind(req.kind.as_str())
    .bind(req.e2ee)
    .bind(req.category_id.map(|c| c.0))
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
    Ok(Json(guild_for(&state, guild, auth.id).await?))
}

/// Changes the name, AFK settings or voice bitrate (`MANAGE_GUILD`).
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
    if let Some(bitrate) = req.audio_bitrate {
        if !AUDIO_BITRATES.contains(&bitrate) {
            return Err(ApiError::BadRequest(format!("audio bitrate must be one of {AUDIO_BITRATES:?}")));
        }
        sqlx::query("UPDATE guilds SET audio_bitrate = $2 WHERE id = $1")
            .bind(guild_id.0)
            .bind(bitrate as i32)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(share) = req.share_history {
        sqlx::query("UPDATE guilds SET share_history = $2 WHERE id = $1")
            .bind(guild_id.0)
            .bind(share)
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
    if user_id != auth.id {
        require_outranks(&state, guild_id, auth.id, user_id).await?;
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

/// Members can only kick or ban members whose highest role is below their own.
async fn require_outranks(state: &AppState, guild_id: GuildId, actor: UserId, target: UserId) -> ApiResult<()> {
    let me = db::membership(&state.db, guild_id, actor).await?.ok_or(ApiError::NotFound)?;
    // Someone who already left can still be banned.
    let rank = db::membership(&state.db, guild_id, target).await?.map_or(0, |m| m.top_position);
    if me.outranks(rank) { Ok(()) } else { Err(ApiError::Forbidden) }
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
    require_outranks(&state, guild_id, auth.id, user_id).await?;
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

/// Everyone who can read a channel: guild members who can see it, or the
/// DM's members. Clients use it to build the channel's MLS group.
pub async fn channel_members(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
) -> ApiResult<Json<Vec<User>>> {
    let channel = db::require_channel(&state.db, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    let ids = db::channel_viewers(&state.db, &channel).await?;
    Ok(Json(db::users(&state.db, &ids).await?))
}
