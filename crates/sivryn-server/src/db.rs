//! Database rows and shared queries.

use chrono::{DateTime, Utc};
use sivryn_protocol::model::{Channel, ChannelKind, Guild, Message, User};
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{ChannelId, DeviceId, GuildId, MessageId, UserId};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

#[derive(sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub password_hash: Option<String>,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        User { id: UserId(r.id), username: r.username, display_name: r.display_name, created_at: r.created_at }
    }
}

#[derive(sqlx::FromRow)]
pub struct GuildRow {
    pub id: Uuid,
    pub name: String,
    pub owner_id: Uuid,
    pub created_at: DateTime<Utc>,
}

impl From<GuildRow> for Guild {
    fn from(r: GuildRow) -> Self {
        Guild { id: GuildId(r.id), name: r.name, owner_id: UserId(r.owner_id), created_at: r.created_at }
    }
}

#[derive(sqlx::FromRow)]
pub struct ChannelRow {
    pub id: Uuid,
    pub guild_id: Uuid,
    pub name: String,
    pub kind: String,
    pub position: i32,
    pub e2ee: bool,
}

impl From<ChannelRow> for Channel {
    fn from(r: ChannelRow) -> Self {
        Channel {
            id: ChannelId(r.id),
            guild_id: GuildId(r.guild_id),
            name: r.name,
            kind: ChannelKind::parse(&r.kind).unwrap_or(ChannelKind::Text),
            position: r.position,
            e2ee: r.e2ee,
        }
    }
}

#[derive(sqlx::FromRow)]
pub struct MessageRow {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub author_id: Uuid,
    pub sender_device_id: Uuid,
    pub epoch: i64,
    pub ciphertext: String,
    pub created_at: DateTime<Utc>,
}

impl From<MessageRow> for Message {
    fn from(r: MessageRow) -> Self {
        Message {
            id: MessageId(r.id),
            channel_id: ChannelId(r.channel_id),
            author_id: UserId(r.author_id),
            sender_device_id: DeviceId(r.sender_device_id),
            epoch: r.epoch as u64,
            ciphertext: r.ciphertext,
            created_at: r.created_at,
        }
    }
}

pub async fn user(pool: &PgPool, id: UserId) -> ApiResult<UserRow> {
    Ok(sqlx::query_as("SELECT * FROM users WHERE id = $1").bind(id.0).fetch_one(pool).await?)
}

pub async fn channel(pool: &PgPool, id: ChannelId) -> ApiResult<ChannelRow> {
    Ok(sqlx::query_as("SELECT * FROM channels WHERE id = $1").bind(id.0).fetch_one(pool).await?)
}

pub async fn user_guilds(pool: &PgPool, user: UserId) -> ApiResult<Vec<Guild>> {
    let rows: Vec<GuildRow> = sqlx::query_as(
        "SELECT g.* FROM guilds g JOIN guild_members m ON m.guild_id = g.id
         WHERE m.user_id = $1 ORDER BY m.joined_at",
    )
    .bind(user.0)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Effective permissions of `user` in `guild`, or `None` if not a member.
pub async fn guild_permissions(pool: &PgPool, guild: GuildId, user: UserId) -> ApiResult<Option<Permissions>> {
    let row: Option<(Uuid, i64)> = sqlx::query_as(
        "SELECT g.owner_id, COALESCE(bit_or(r.permissions), 0)::BIGINT
         FROM guild_members m
         JOIN guilds g ON g.id = m.guild_id
         LEFT JOIN roles r ON r.guild_id = m.guild_id AND (
             r.is_everyone OR r.id IN (
                 SELECT role_id FROM member_roles mr WHERE mr.guild_id = m.guild_id AND mr.user_id = m.user_id
             )
         )
         WHERE m.guild_id = $1 AND m.user_id = $2
         GROUP BY g.owner_id",
    )
    .bind(guild.0)
    .bind(user.0)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(owner, bits)| {
            if owner == user.0 { Permissions::all() } else { Permissions::from_bits_truncate(bits as u64) }
        },
    ))
}

/// Ensures `user` is a member of `guild` with `required` permissions.
pub async fn require(pool: &PgPool, guild: GuildId, user: UserId, required: Permissions) -> ApiResult<()> {
    match guild_permissions(pool, guild, user).await? {
        // Non-members get 404 so guild IDs can't be probed.
        None => Err(ApiError::NotFound),
        Some(p) if p.allows(required) => Ok(()),
        Some(_) => Err(ApiError::Forbidden),
    }
}

/// Loads a channel and checks that `user` may use it with `required` permissions.
pub async fn require_channel(
    pool: &PgPool,
    channel_id: ChannelId,
    user: UserId,
    required: Permissions,
) -> ApiResult<ChannelRow> {
    let channel = channel(pool, channel_id).await?;
    require(pool, GuildId(channel.guild_id), user, required).await?;
    Ok(channel)
}

/// Ensures `device` belongs to `user`.
pub async fn require_own_device(pool: &PgPool, device: DeviceId, user: UserId) -> ApiResult<()> {
    let owned: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM devices WHERE id = $1 AND user_id = $2")
        .bind(device.0)
        .bind(user.0)
        .fetch_optional(pool)
        .await?;
    owned.map(|_| ()).ok_or(ApiError::Forbidden)
}
