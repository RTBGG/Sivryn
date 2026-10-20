//! Database rows and shared queries.

use chrono::{DateTime, Utc};
use sivryn_protocol::gateway::Audience;
use sivryn_protocol::model::{Category, Channel, ChannelKind, Guild, Message, MessageKind, User, UserStatus};
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{CategoryId, ChannelId, DeviceId, FileId, GuildId, MessageId, UserId};
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
    pub allow_server_dms: bool,
    pub bio: String,
    pub avatar_id: Option<Uuid>,
    pub banner_id: Option<Uuid>,
    pub status: String,
}

impl UserRow {
    pub fn status(&self) -> UserStatus {
        UserStatus::parse(&self.status)
    }
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        User {
            id: UserId(r.id),
            username: r.username,
            display_name: r.display_name,
            created_at: r.created_at,
            avatar_id: r.avatar_id.map(FileId),
        }
    }
}

#[derive(sqlx::FromRow)]
pub struct GuildRow {
    pub id: Uuid,
    pub name: String,
    pub owner_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub icon_id: Option<Uuid>,
    pub afk_channel_id: Option<Uuid>,
    pub afk_timeout_minutes: i32,
    pub audio_bitrate: i32,
}

impl From<GuildRow> for Guild {
    fn from(r: GuildRow) -> Self {
        Guild {
            id: GuildId(r.id),
            name: r.name,
            owner_id: UserId(r.owner_id),
            created_at: r.created_at,
            icon_id: r.icon_id.map(FileId),
            afk_channel_id: r.afk_channel_id.map(ChannelId),
            afk_timeout_minutes: r.afk_timeout_minutes.max(1) as u32,
            audio_bitrate: r.audio_bitrate.max(8000) as u32,
        }
    }
}

#[derive(sqlx::FromRow)]
pub struct ChannelRow {
    pub id: Uuid,
    pub guild_id: Option<Uuid>,
    pub name: String,
    pub kind: String,
    pub position: i32,
    pub e2ee: bool,
    pub created_at: DateTime<Utc>,
    pub category_id: Option<Uuid>,
}

impl ChannelRow {
    pub fn kind(&self) -> ChannelKind {
        ChannelKind::parse(&self.kind).unwrap_or(ChannelKind::Text)
    }

    pub fn guild(&self) -> Option<GuildId> {
        self.guild_id.map(GuildId)
    }
}

impl From<ChannelRow> for Channel {
    fn from(r: ChannelRow) -> Self {
        Channel {
            id: ChannelId(r.id),
            guild_id: r.guild_id.map(GuildId),
            kind: r.kind(),
            name: r.name,
            position: r.position,
            e2ee: r.e2ee,
            category_id: r.category_id.map(CategoryId),
        }
    }
}

#[derive(sqlx::FromRow)]
pub struct CategoryRow {
    pub id: Uuid,
    pub guild_id: Uuid,
    pub name: String,
    pub position: i32,
}

impl From<CategoryRow> for Category {
    fn from(r: CategoryRow) -> Self {
        Category { id: CategoryId(r.id), guild_id: GuildId(r.guild_id), name: r.name, position: r.position }
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
    pub kind: String,
    pub target_id: Option<Uuid>,
}

impl From<MessageRow> for Message {
    fn from(r: MessageRow) -> Self {
        Message {
            id: MessageId(r.id),
            channel_id: ChannelId(r.channel_id),
            author_id: UserId(r.author_id),
            sender_device_id: DeviceId(r.sender_device_id),
            kind: MessageKind::parse(&r.kind),
            target_id: r.target_id.map(MessageId),
            epoch: r.epoch as u64,
            ciphertext: r.ciphertext,
            created_at: r.created_at,
        }
    }
}

pub async fn user(pool: &PgPool, id: UserId) -> ApiResult<UserRow> {
    Ok(sqlx::query_as("SELECT * FROM users WHERE id = $1").bind(id.0).fetch_one(pool).await?)
}

pub async fn users(pool: &PgPool, ids: &[UserId]) -> ApiResult<Vec<User>> {
    let ids: Vec<Uuid> = ids.iter().map(|u| u.0).collect();
    let rows: Vec<UserRow> = sqlx::query_as("SELECT * FROM users WHERE id = ANY($1) ORDER BY lower(username)")
        .bind(&ids)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn guild(pool: &PgPool, id: GuildId) -> ApiResult<Guild> {
    let row: GuildRow = sqlx::query_as("SELECT * FROM guilds WHERE id = $1").bind(id.0).fetch_one(pool).await?;
    Ok(row.into())
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

/// What members of a DM or group may do: everything except managing guilds.
fn private_permissions() -> Permissions {
    Permissions::VIEW_CHANNEL
        | Permissions::SEND_MESSAGES
        | Permissions::ATTACH_FILES
        | Permissions::CONNECT
        | Permissions::SPEAK
        | Permissions::VIDEO
        | Permissions::SCREEN_SHARE
}

pub async fn is_channel_member(pool: &PgPool, channel: ChannelId, user: UserId) -> ApiResult<bool> {
    let (member,): (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM channel_members WHERE channel_id = $1 AND user_id = $2)")
            .bind(channel.0)
            .bind(user.0)
            .fetch_one(pool)
            .await?;
    Ok(member)
}

/// Loads a channel and checks that `user` may use it with `required`
/// permissions: guild permissions for guild channels, membership for DMs.
pub async fn require_channel(
    pool: &PgPool,
    channel_id: ChannelId,
    user: UserId,
    required: Permissions,
) -> ApiResult<ChannelRow> {
    let channel = channel(pool, channel_id).await?;
    match channel.guild() {
        Some(guild) => require(pool, guild, user, required).await?,
        None => {
            if !is_channel_member(pool, channel_id, user).await? {
                return Err(ApiError::NotFound);
            }
            if !private_permissions().contains(required) {
                return Err(ApiError::Forbidden);
            }
        }
    }
    Ok(channel)
}

pub async fn channel_member_ids(pool: &PgPool, channel: ChannelId) -> ApiResult<Vec<UserId>> {
    let rows: Vec<(Uuid,)> = sqlx::query_as("SELECT user_id FROM channel_members WHERE channel_id = $1")
        .bind(channel.0)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|(id,)| UserId(id)).collect())
}

/// Who receives events about a channel.
pub async fn channel_audience(pool: &PgPool, channel: &ChannelRow) -> ApiResult<Audience> {
    Ok(match channel.guild() {
        Some(guild) => Audience::Guild(guild),
        None => Audience::Users(channel_member_ids(pool, ChannelId(channel.id)).await?),
    })
}

/// Users whose presence `user` may see: friends and people sharing a guild.
pub async fn related_users(pool: &PgPool, user: UserId) -> ApiResult<Vec<UserId>> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT friend_id FROM friends WHERE user_id = $1
         UNION
         SELECT b.user_id FROM guild_members a JOIN guild_members b ON a.guild_id = b.guild_id
         WHERE a.user_id = $1 AND b.user_id <> $1
         UNION
         SELECT b.user_id FROM channel_members a JOIN channel_members b ON a.channel_id = b.channel_id
         WHERE a.user_id = $1 AND b.user_id <> $1",
    )
    .bind(user.0)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| UserId(id)).collect())
}

pub async fn are_friends(pool: &PgPool, a: UserId, b: UserId) -> ApiResult<bool> {
    let (yes,): (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM friends WHERE user_id = $1 AND friend_id = $2)")
        .bind(a.0)
        .bind(b.0)
        .fetch_one(pool)
        .await?;
    Ok(yes)
}

pub async fn share_guild(pool: &PgPool, a: UserId, b: UserId) -> ApiResult<bool> {
    let (yes,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (
             SELECT 1 FROM guild_members x JOIN guild_members y ON x.guild_id = y.guild_id
             WHERE x.user_id = $1 AND y.user_id = $2
         )",
    )
    .bind(a.0)
    .bind(b.0)
    .fetch_one(pool)
    .await?;
    Ok(yes)
}

/// Whether either user blocked the other.
pub async fn blocked_either_way(pool: &PgPool, a: UserId, b: UserId) -> ApiResult<bool> {
    let (yes,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (
             SELECT 1 FROM blocks
             WHERE (user_id = $1 AND blocked_id = $2) OR (user_id = $2 AND blocked_id = $1)
         )",
    )
    .bind(a.0)
    .bind(b.0)
    .fetch_one(pool)
    .await?;
    Ok(yes)
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
