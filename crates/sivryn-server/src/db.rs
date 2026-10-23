//! Database rows and shared queries.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sivryn_protocol::gateway::Audience;
use sivryn_protocol::model::{
    Category, Channel, ChannelKind, Guild, Message, MessageKind, OverwriteTarget, PermissionOverwrite, Role, User,
    UserStatus,
};
use sivryn_protocol::permissions::{Permissions, channel_permissions};
use sivryn_protocol::{CategoryId, ChannelId, DeviceId, FileId, GuildId, MessageId, RoleId, UserId};
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
    pub share_history: bool,
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
            share_history: r.share_history,
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
            overwrites: Vec::new(),
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
        Category {
            id: CategoryId(r.id),
            guild_id: GuildId(r.guild_id),
            name: r.name,
            position: r.position,
            overwrites: Vec::new(),
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
    pub kind: String,
    pub target_id: Option<Uuid>,
    pub history: Option<String>,
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
            history: r.history,
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

/// Effective guild-wide permissions of `user` in `guild`, or `None` if not a member.
pub async fn guild_permissions(pool: &PgPool, guild: GuildId, user: UserId) -> ApiResult<Option<Permissions>> {
    Ok(membership(pool, guild, user).await?.map(|m| m.permissions))
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
/// permissions: their permissions in that channel for guild channels,
/// membership for DMs. Channels they can't see answer 404.
pub async fn require_channel(
    pool: &PgPool,
    channel_id: ChannelId,
    user: UserId,
    required: Permissions,
) -> ApiResult<ChannelRow> {
    let channel = channel(pool, channel_id).await?;
    match channel.guild() {
        Some(_) => match channel_permissions_of(pool, &channel, user).await? {
            Some(p) if p.allows(Permissions::VIEW_CHANNEL) => {
                if !p.allows(required) {
                    return Err(ApiError::Forbidden);
                }
            }
            _ => return Err(ApiError::NotFound),
        },
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

/// Who receives events about a channel: the whole guild, unless overwrites
/// may hide the channel from some members; then only those who can see it.
pub async fn channel_audience(pool: &PgPool, channel: &ChannelRow) -> ApiResult<Audience> {
    Ok(match channel.guild() {
        Some(guild) => {
            let overwrites = guild_overwrites(pool, guild).await?;
            if overwrites.layers(channel).iter().all(|l| l.is_empty()) {
                Audience::Guild(guild)
            } else {
                Audience::Users(channel_viewers(pool, channel).await?)
            }
        }
        None => Audience::Users(channel_member_ids(pool, ChannelId(channel.id)).await?),
    })
}

// --- Roles and channel permissions ---------------------------------------------

#[derive(sqlx::FromRow)]
pub struct RoleRow {
    pub id: Uuid,
    pub guild_id: Uuid,
    pub name: String,
    pub permissions: i64,
    pub position: i32,
    pub is_everyone: bool,
    pub color: Option<i32>,
    pub hoist: bool,
}

impl From<RoleRow> for Role {
    fn from(r: RoleRow) -> Self {
        Role {
            id: RoleId(r.id),
            guild_id: GuildId(r.guild_id),
            name: r.name,
            permissions: Permissions::from_bits_truncate(r.permissions as u64),
            position: r.position,
            color: r.color.map(|c| c as u32 & 0xff_ffff),
            hoist: r.hoist,
            is_everyone: r.is_everyone,
        }
    }
}

/// A guild's roles, highest first, `@everyone` last.
pub async fn roles(pool: &PgPool, guild: GuildId) -> ApiResult<Vec<Role>> {
    let rows: Vec<RoleRow> = sqlx::query_as(
        "SELECT id, guild_id, name, permissions, position, is_everyone, color, hoist FROM roles
         WHERE guild_id = $1 ORDER BY is_everyone, position DESC, created_at",
    )
    .bind(guild.0)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn role(pool: &PgPool, id: RoleId) -> ApiResult<Role> {
    let row: RoleRow = sqlx::query_as(
        "SELECT id, guild_id, name, permissions, position, is_everyone, color, hoist FROM roles WHERE id = $1",
    )
    .bind(id.0)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)?;
    Ok(row.into())
}

#[derive(sqlx::FromRow)]
struct OverwriteRow {
    channel_id: Option<Uuid>,
    category_id: Option<Uuid>,
    role_id: Option<Uuid>,
    user_id: Option<Uuid>,
    allow_bits: i64,
    deny_bits: i64,
}

/// All permission overwrites of a guild, by channel and by category.
#[derive(Default)]
pub struct GuildOverwrites {
    pub channels: HashMap<ChannelId, Vec<PermissionOverwrite>>,
    pub categories: HashMap<CategoryId, Vec<PermissionOverwrite>>,
}

impl GuildOverwrites {
    /// The overwrites that apply to a channel, category first.
    pub fn layers(&self, channel: &ChannelRow) -> [&[PermissionOverwrite]; 2] {
        let category = channel.category_id.and_then(|c| self.categories.get(&CategoryId(c)));
        let own = self.channels.get(&ChannelId(channel.id));
        [category.map_or(&[][..], Vec::as_slice), own.map_or(&[][..], Vec::as_slice)]
    }
}

pub async fn guild_overwrites(pool: &PgPool, guild: GuildId) -> ApiResult<GuildOverwrites> {
    let rows: Vec<OverwriteRow> = sqlx::query_as(
        "SELECT channel_id, category_id, role_id, user_id, allow_bits, deny_bits
         FROM permission_overwrites WHERE guild_id = $1",
    )
    .bind(guild.0)
    .fetch_all(pool)
    .await?;
    let mut out = GuildOverwrites::default();
    for r in rows {
        let target = match (r.role_id, r.user_id) {
            (Some(role), _) => OverwriteTarget::Role(RoleId(role)),
            (None, Some(user)) => OverwriteTarget::Member(UserId(user)),
            (None, None) => continue,
        };
        let overwrite = PermissionOverwrite {
            target,
            allow: Permissions::from_bits_truncate(r.allow_bits as u64),
            deny: Permissions::from_bits_truncate(r.deny_bits as u64),
        };
        match (r.channel_id, r.category_id) {
            (Some(c), _) => out.channels.entry(ChannelId(c)).or_default().push(overwrite),
            (None, Some(c)) => out.categories.entry(CategoryId(c)).or_default().push(overwrite),
            (None, None) => {}
        }
    }
    Ok(out)
}

/// A member's standing in a guild.
#[derive(Clone, Debug)]
pub struct Membership {
    pub owner: bool,
    pub everyone: RoleId,
    /// Roles besides `@everyone`.
    pub roles: Vec<RoleId>,
    /// Guild-wide permissions of all their roles together.
    pub permissions: Permissions,
    /// Position of their highest role (0 with only `@everyone`).
    pub top_position: i32,
}

impl Membership {
    /// Their permissions in one channel of this guild.
    pub fn in_channel(&self, user: UserId, overwrites: &GuildOverwrites, channel: &ChannelRow) -> Permissions {
        if self.owner {
            return Permissions::all();
        }
        channel_permissions(self.permissions, self.everyone, &self.roles, user, &overwrites.layers(channel))
    }

    /// Their permissions for a category itself (whether it shows up).
    pub fn in_category(&self, user: UserId, overwrites: &GuildOverwrites, category: CategoryId) -> Permissions {
        if self.owner {
            return Permissions::all();
        }
        let layer = overwrites.categories.get(&category).map_or(&[][..], Vec::as_slice);
        channel_permissions(self.permissions, self.everyone, &self.roles, user, &[layer])
    }

    /// Whether they may manage a role at `position`, or a member whose highest
    /// role is there: only below their own highest role, unless they own the guild.
    pub fn outranks(&self, position: i32) -> bool {
        self.owner || self.top_position > position
    }
}

/// Every member of a guild with their standing.
pub async fn memberships(pool: &PgPool, guild: GuildId) -> ApiResult<Vec<(UserId, Membership)>> {
    let (owner,): (Uuid,) =
        sqlx::query_as("SELECT owner_id FROM guilds WHERE id = $1").bind(guild.0).fetch_one(pool).await?;
    let roles = roles(pool, guild).await?;
    let everyone = roles.iter().find(|r| r.is_everyone).ok_or(ApiError::NotFound)?;
    let members: Vec<(Uuid,)> =
        sqlx::query_as("SELECT user_id FROM guild_members WHERE guild_id = $1").bind(guild.0).fetch_all(pool).await?;
    let assigned: Vec<(Uuid, Uuid)> = sqlx::query_as("SELECT user_id, role_id FROM member_roles WHERE guild_id = $1")
        .bind(guild.0)
        .fetch_all(pool)
        .await?;
    Ok(members
        .into_iter()
        .map(|(user,)| {
            let mine: Vec<&Role> = assigned
                .iter()
                .filter(|(u, _)| *u == user)
                .filter_map(|(_, r)| roles.iter().find(|role| role.id.0 == *r))
                .collect();
            let permissions = mine.iter().fold(everyone.permissions, |p, r| p | r.permissions);
            let membership = Membership {
                owner: user == owner,
                everyone: everyone.id,
                roles: mine.iter().map(|r| r.id).collect(),
                permissions: if user == owner { Permissions::all() } else { permissions },
                top_position: mine.iter().map(|r| r.position).max().unwrap_or(0),
            };
            (UserId(user), membership)
        })
        .collect())
}

/// One member's standing, or `None` if they aren't a member.
pub async fn membership(pool: &PgPool, guild: GuildId, user: UserId) -> ApiResult<Option<Membership>> {
    let owner: Option<(Uuid,)> = sqlx::query_as(
        "SELECT g.owner_id FROM guild_members m JOIN guilds g ON g.id = m.guild_id
         WHERE m.guild_id = $1 AND m.user_id = $2",
    )
    .bind(guild.0)
    .bind(user.0)
    .fetch_optional(pool)
    .await?;
    let Some((owner,)) = owner else { return Ok(None) };
    let rows: Vec<(Uuid, i64, i32, bool)> = sqlx::query_as(
        "SELECT id, permissions, position, is_everyone FROM roles
         WHERE guild_id = $1 AND (is_everyone OR id IN (
             SELECT role_id FROM member_roles WHERE guild_id = $1 AND user_id = $2
         ))",
    )
    .bind(guild.0)
    .bind(user.0)
    .fetch_all(pool)
    .await?;
    let everyone = rows.iter().find(|r| r.3).map(|r| RoleId(r.0)).ok_or(ApiError::NotFound)?;
    let permissions = rows.iter().fold(Permissions::empty(), |p, r| p | Permissions::from_bits_truncate(r.1 as u64));
    let owner = owner == user.0;
    Ok(Some(Membership {
        owner,
        everyone,
        roles: rows.iter().filter(|r| !r.3).map(|r| RoleId(r.0)).collect(),
        permissions: if owner { Permissions::all() } else { permissions },
        top_position: rows.iter().filter(|r| !r.3).map(|r| r.2).max().unwrap_or(0),
    }))
}

/// A member's permissions in a guild channel, or `None` if not a member.
pub async fn channel_permissions_of(
    pool: &PgPool,
    channel: &ChannelRow,
    user: UserId,
) -> ApiResult<Option<Permissions>> {
    let Some(guild) = channel.guild() else { return Ok(None) };
    let Some(member) = membership(pool, guild, user).await? else { return Ok(None) };
    let overwrites = guild_overwrites(pool, guild).await?;
    Ok(Some(member.in_channel(user, &overwrites, channel)))
}

/// Everyone who can see a guild channel (the members of a DM).
pub async fn channel_viewers(pool: &PgPool, channel: &ChannelRow) -> ApiResult<Vec<UserId>> {
    let Some(guild) = channel.guild() else { return channel_member_ids(pool, ChannelId(channel.id)).await };
    let overwrites = guild_overwrites(pool, guild).await?;
    Ok(memberships(pool, guild)
        .await?
        .into_iter()
        .filter(|(user, m)| m.in_channel(*user, &overwrites, channel).allows(Permissions::VIEW_CHANNEL))
        .map(|(user, _)| user)
        .collect())
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
