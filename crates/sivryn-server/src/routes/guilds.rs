use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;
use sivryn_protocol::api::{CreateChannelRequest, CreateGuildRequest, GuildWithChannels};
use sivryn_protocol::gateway::Event;
use sivryn_protocol::model::{Channel, ChannelKind, Guild, User};
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{ChannelId, GuildId, InviteId, RoleId};
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

pub async fn list(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Vec<GuildWithChannels>>> {
    let mut out = Vec::new();
    for guild in db::user_guilds(&state.db, auth.id).await? {
        let channels = channels_of(&state, guild.id).await?;
        out.push(GuildWithChannels { guild, channels });
    }
    Ok(Json(out))
}

pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
) -> ApiResult<Json<GuildWithChannels>> {
    db::require(&state.db, guild_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    let guild: GuildRow =
        sqlx::query_as("SELECT * FROM guilds WHERE id = $1").bind(guild_id.0).fetch_one(&state.db).await?;
    let channels = channels_of(&state, guild_id).await?;
    Ok(Json(GuildWithChannels { guild: guild.into(), channels }))
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
    state.events.publish(Event::GuildCreate { guild: guild.clone() });
    Ok((StatusCode::CREATED, Json(GuildWithChannels { guild, channels })))
}

pub async fn create_channel(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
    Json(req): Json<CreateChannelRequest>,
) -> ApiResult<(StatusCode, Json<Channel>)> {
    db::require(&state.db, guild_id, auth.id, Permissions::MANAGE_CHANNELS).await?;
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
    state.events.publish(Event::ChannelCreate { channel: channel.clone() });
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

    let inserted = sqlx::query("INSERT INTO guild_members (guild_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(guild_id.0)
        .bind(auth.id.0)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    if inserted.rows_affected() > 0 {
        let user = db::user(&state.db, auth.id).await?;
        state.events.publish(Event::MemberJoin { guild_id, user: user.into() });
    }

    let guild: GuildRow =
        sqlx::query_as("SELECT * FROM guilds WHERE id = $1").bind(guild_id.0).fetch_one(&state.db).await?;
    let channels = channels_of(&state, guild_id).await?;
    Ok(Json(GuildWithChannels { guild: guild.into(), channels }))
}
