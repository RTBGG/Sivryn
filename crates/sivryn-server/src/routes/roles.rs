//! Roles, member roles, permission overwrites, the channel layout and
//! moving people between voice channels.
//!
//! Like Discord, members can only manage roles (and members) below their own
//! highest role, and can't hand out permissions they don't have themselves;
//! the guild owner and administrators are exempt from the latter.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sivryn_protocol::api::{LayoutRequest, MoveMemberRequest, RoleOrderRequest, RoleRequest};
use sivryn_protocol::gateway::{Audience, Event};
use sivryn_protocol::model::{ChannelKind, OverwriteTarget, PermissionOverwrite, Role};
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{CategoryId, ChannelId, GuildId, RoleId, UserId};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::db::{self, Membership};
use crate::error::{ApiError, ApiResult};

/// The acting member, if they have `required` in the guild.
async fn actor(state: &AppState, guild: GuildId, user: UserId, required: Permissions) -> ApiResult<Membership> {
    let me = db::membership(&state.db, guild, user).await?.ok_or(ApiError::NotFound)?;
    if !me.permissions.allows(required) {
        return Err(ApiError::Forbidden);
    }
    Ok(me)
}

/// Whether `me` may grant (or take away) `permissions`.
fn may_grant(me: &Membership, permissions: Permissions) -> bool {
    me.owner || me.permissions.contains(Permissions::ADMINISTRATOR) || me.permissions.contains(permissions)
}

fn validate_role_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if (1..=100).contains(&name.chars().count()) {
        Ok(name.to_owned())
    } else {
        Err(ApiError::BadRequest("role names must be 1-100 characters".into()))
    }
}

fn validate_color(color: Option<u32>) -> ApiResult<Option<i32>> {
    match color {
        Some(c) if c > 0xff_ffff => Err(ApiError::BadRequest("colors are 0xRRGGBB".into())),
        c => Ok(c.map(|c| c as i32)),
    }
}

/// Members reload the guild; devices fix up channel groups whose readers changed.
async fn access_changed(state: &AppState, guild_id: GuildId) -> ApiResult<()> {
    let guild = db::guild(&state.db, guild_id).await?;
    state.events.publish(Audience::Guild(guild_id), Event::GuildUpdate { guild });
    state.events.publish(Audience::Guild(guild_id), Event::ChannelAccessChanged { guild_id });
    Ok(())
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
) -> ApiResult<Json<Vec<Role>>> {
    db::require(&state.db, guild_id, auth.id, Permissions::empty()).await?;
    Ok(Json(db::roles(&state.db, guild_id).await?))
}

/// Creates a role just above `@everyone` (`MANAGE_ROLES`).
pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
    Json(req): Json<RoleRequest>,
) -> ApiResult<(StatusCode, Json<Role>)> {
    let me = actor(&state, guild_id, auth.id, Permissions::MANAGE_ROLES).await?;
    let permissions = req.permissions.unwrap_or(Permissions::empty());
    if !may_grant(&me, permissions) {
        return Err(ApiError::Forbidden);
    }
    let name = validate_role_name(req.name.as_deref().unwrap_or("new role"))?;
    let color = validate_color(req.color.flatten())?;
    let id = RoleId::new();
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE roles SET position = position + 1 WHERE guild_id = $1 AND NOT is_everyone")
        .bind(guild_id.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO roles (id, guild_id, name, permissions, position, color, hoist) VALUES ($1, $2, $3, $4, 1, $5, $6)",
    )
    .bind(id.0)
    .bind(guild_id.0)
    .bind(name)
    .bind(permissions.bits() as i64)
    .bind(color)
    .bind(req.hoist.unwrap_or(false))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    access_changed(&state, guild_id).await?;
    Ok((StatusCode::CREATED, Json(db::role(&state.db, id).await?)))
}

/// Changes a role below your own highest one. `@everyone` only has permissions.
pub async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(role_id): Path<RoleId>,
    Json(req): Json<RoleRequest>,
) -> ApiResult<Json<Role>> {
    let role = db::role(&state.db, role_id).await?;
    let me = actor(&state, role.guild_id, auth.id, Permissions::MANAGE_ROLES).await?;
    if !me.outranks(role.position) {
        return Err(ApiError::Forbidden);
    }
    if role.is_everyone && (req.name.is_some() || req.color.is_some() || req.hoist.is_some()) {
        return Err(ApiError::BadRequest("@everyone only has permissions".into()));
    }
    let mut tx = state.db.begin().await?;
    if let Some(permissions) = req.permissions {
        // Adding or removing a permission both need it.
        if !may_grant(&me, permissions ^ role.permissions) {
            return Err(ApiError::Forbidden);
        }
        sqlx::query("UPDATE roles SET permissions = $2 WHERE id = $1")
            .bind(role_id.0)
            .bind(permissions.bits() as i64)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(name) = &req.name {
        sqlx::query("UPDATE roles SET name = $2 WHERE id = $1")
            .bind(role_id.0)
            .bind(validate_role_name(name)?)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(color) = req.color {
        sqlx::query("UPDATE roles SET color = $2 WHERE id = $1")
            .bind(role_id.0)
            .bind(validate_color(color)?)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(hoist) = req.hoist {
        sqlx::query("UPDATE roles SET hoist = $2 WHERE id = $1").bind(role_id.0).bind(hoist).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    access_changed(&state, role.guild_id).await?;
    Ok(Json(db::role(&state.db, role_id).await?))
}

pub async fn delete(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(role_id): Path<RoleId>,
) -> ApiResult<StatusCode> {
    let role = db::role(&state.db, role_id).await?;
    let me = actor(&state, role.guild_id, auth.id, Permissions::MANAGE_ROLES).await?;
    if role.is_everyone {
        return Err(ApiError::BadRequest("@everyone can't be deleted".into()));
    }
    if !me.outranks(role.position) {
        return Err(ApiError::Forbidden);
    }
    sqlx::query("DELETE FROM roles WHERE id = $1").bind(role_id.0).execute(&state.db).await?;
    access_changed(&state, role.guild_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Reorders all roles (highest first). Roles at or above your own highest
/// role keep their place.
pub async fn reorder(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
    Json(req): Json<RoleOrderRequest>,
) -> ApiResult<StatusCode> {
    let me = actor(&state, guild_id, auth.id, Permissions::MANAGE_ROLES).await?;
    let roles: Vec<Role> = db::roles(&state.db, guild_id).await?.into_iter().filter(|r| !r.is_everyone).collect();
    let mut sorted = req.roles.clone();
    sorted.sort_by_key(|r| r.0);
    sorted.dedup();
    if sorted.len() != req.roles.len()
        || sorted.len() != roles.len()
        || !roles.iter().all(|r| req.roles.contains(&r.id))
    {
        return Err(ApiError::BadRequest("list every role exactly once".into()));
    }
    let count = req.roles.len() as i32;
    let new_position = |id: RoleId| count - req.roles.iter().position(|r| *r == id).unwrap_or(0) as i32;
    if roles.iter().any(|r| !me.outranks(r.position) && new_position(r.id) != r.position) {
        return Err(ApiError::Forbidden);
    }
    // Nothing may move above your own highest role either.
    if roles.iter().any(|r| me.outranks(r.position) && !me.outranks(new_position(r.id))) {
        return Err(ApiError::Forbidden);
    }
    let mut tx = state.db.begin().await?;
    for role in &roles {
        sqlx::query("UPDATE roles SET position = $2 WHERE id = $1")
            .bind(role.id.0)
            .bind(new_position(role.id))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    access_changed(&state, guild_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The role, if it belongs to the guild and may be given or taken by `me`.
async fn assignable(state: &AppState, me: &Membership, guild_id: GuildId, role_id: RoleId) -> ApiResult<Role> {
    let role = db::role(&state.db, role_id).await?;
    if role.guild_id != guild_id || role.is_everyone {
        return Err(ApiError::NotFound);
    }
    if !me.outranks(role.position) {
        return Err(ApiError::Forbidden);
    }
    Ok(role)
}

pub async fn add_member_role(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((guild_id, user_id, role_id)): Path<(GuildId, UserId, RoleId)>,
) -> ApiResult<StatusCode> {
    let me = actor(&state, guild_id, auth.id, Permissions::MANAGE_ROLES).await?;
    assignable(&state, &me, guild_id, role_id).await?;
    let added = sqlx::query(
        "INSERT INTO member_roles (guild_id, user_id, role_id)
         SELECT $1, $2, $3 WHERE EXISTS (SELECT 1 FROM guild_members WHERE guild_id = $1 AND user_id = $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(guild_id.0)
    .bind(user_id.0)
    .bind(role_id.0)
    .execute(&state.db)
    .await?;
    if added.rows_affected() > 0 {
        access_changed(&state, guild_id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_member_role(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((guild_id, user_id, role_id)): Path<(GuildId, UserId, RoleId)>,
) -> ApiResult<StatusCode> {
    let me = actor(&state, guild_id, auth.id, Permissions::MANAGE_ROLES).await?;
    assignable(&state, &me, guild_id, role_id).await?;
    let removed = sqlx::query("DELETE FROM member_roles WHERE guild_id = $1 AND user_id = $2 AND role_id = $3")
        .bind(guild_id.0)
        .bind(user_id.0)
        .bind(role_id.0)
        .execute(&state.db)
        .await?;
    if removed.rows_affected() > 0 {
        access_changed(&state, guild_id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

// --- Permission overwrites ---------------------------------------------------------

/// Where an overwrite lives.
#[derive(Clone, Copy)]
enum Place {
    Channel(ChannelId),
    Category(CategoryId),
}

impl Place {
    async fn guild(self, state: &AppState) -> ApiResult<GuildId> {
        let row: Option<(Option<Uuid>,)> = match self {
            Place::Channel(c) => {
                sqlx::query_as("SELECT guild_id FROM channels WHERE id = $1")
                    .bind(c.0)
                    .fetch_optional(&state.db)
                    .await?
            }
            Place::Category(c) => sqlx::query_as("SELECT guild_id FROM categories WHERE id = $1")
                .bind(c.0)
                .fetch_optional(&state.db)
                .await?
                .map(|(g,): (Uuid,)| (Some(g),)),
        };
        row.and_then(|(g,)| g).map(GuildId).ok_or(ApiError::NotFound)
    }

    fn ids(self) -> (Option<Uuid>, Option<Uuid>) {
        match self {
            Place::Channel(c) => (Some(c.0), None),
            Place::Category(c) => (None, Some(c.0)),
        }
    }
}

fn parse_target(kind: &str, id: Uuid) -> ApiResult<OverwriteTarget> {
    match kind {
        "role" => Ok(OverwriteTarget::Role(RoleId(id))),
        "member" => Ok(OverwriteTarget::Member(UserId(id))),
        _ => Err(ApiError::BadRequest("target is role or member".into())),
    }
}

/// Checks that `me` may change overwrites for `target` in the guild.
async fn check_target(state: &AppState, me: &Membership, guild_id: GuildId, target: OverwriteTarget) -> ApiResult<()> {
    match target {
        OverwriteTarget::Role(role_id) => {
            let role = db::role(&state.db, role_id).await?;
            if role.guild_id != guild_id {
                return Err(ApiError::NotFound);
            }
            if !role.is_everyone && !me.outranks(role.position) {
                return Err(ApiError::Forbidden);
            }
        }
        OverwriteTarget::Member(user) => {
            db::membership(&state.db, guild_id, user).await?.ok_or(ApiError::NotFound)?;
        }
    }
    Ok(())
}

/// Replaces the overwrite for one target in a channel or category; an empty
/// one is removed.
async fn write_overwrite(
    state: &AppState,
    auth: UserId,
    place: Place,
    overwrite: PermissionOverwrite,
) -> ApiResult<()> {
    let guild_id = place.guild(state).await?;
    let me = actor(state, guild_id, auth, Permissions::MANAGE_ROLES).await?;
    check_target(state, &me, guild_id, overwrite.target).await?;
    if !may_grant(&me, overwrite.allow | overwrite.deny) {
        return Err(ApiError::Forbidden);
    }
    let (channel, category) = place.ids();
    let (role, user) = match overwrite.target {
        OverwriteTarget::Role(r) => (Some(r.0), None),
        OverwriteTarget::Member(u) => (None, Some(u.0)),
    };
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "DELETE FROM permission_overwrites
         WHERE COALESCE(channel_id, category_id) = $1 AND COALESCE(role_id, user_id) = $2",
    )
    .bind(channel.or(category))
    .bind(role.or(user))
    .execute(&mut *tx)
    .await?;
    let (allow, deny) = (overwrite.allow - overwrite.deny, overwrite.deny);
    if !(allow | deny).is_empty() {
        sqlx::query(
            "INSERT INTO permission_overwrites (guild_id, channel_id, category_id, role_id, user_id, allow_bits, deny_bits)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(guild_id.0)
        .bind(channel)
        .bind(category)
        .bind(role)
        .bind(user)
        .bind(allow.bits() as i64)
        .bind(deny.bits() as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    access_changed(state, guild_id).await
}

pub async fn set_channel_overwrite(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
    Json(overwrite): Json<PermissionOverwrite>,
) -> ApiResult<StatusCode> {
    write_overwrite(&state, auth.id, Place::Channel(channel_id), overwrite).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_channel_overwrite(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((channel_id, kind, id)): Path<(ChannelId, String, Uuid)>,
) -> ApiResult<StatusCode> {
    let target = parse_target(&kind, id)?;
    let empty = PermissionOverwrite { target, allow: Permissions::empty(), deny: Permissions::empty() };
    write_overwrite(&state, auth.id, Place::Channel(channel_id), empty).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn set_category_overwrite(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(category_id): Path<CategoryId>,
    Json(overwrite): Json<PermissionOverwrite>,
) -> ApiResult<StatusCode> {
    write_overwrite(&state, auth.id, Place::Category(category_id), overwrite).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_category_overwrite(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((category_id, kind, id)): Path<(CategoryId, String, Uuid)>,
) -> ApiResult<StatusCode> {
    let target = parse_target(&kind, id)?;
    let empty = PermissionOverwrite { target, allow: Permissions::empty(), deny: Permissions::empty() };
    write_overwrite(&state, auth.id, Place::Category(category_id), empty).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- Channel layout and voice moves -------------------------------------------------

/// Sets the order of categories and channels, and which category each
/// channel is in (`MANAGE_CHANNELS`).
pub async fn layout(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
    Json(req): Json<LayoutRequest>,
) -> ApiResult<StatusCode> {
    actor(&state, guild_id, auth.id, Permissions::MANAGE_CHANNELS).await?;
    let categories: Vec<(Uuid,)> =
        sqlx::query_as("SELECT id FROM categories WHERE guild_id = $1").bind(guild_id.0).fetch_all(&state.db).await?;
    let channels: Vec<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT id, category_id FROM channels WHERE guild_id = $1")
            .bind(guild_id.0)
            .fetch_all(&state.db)
            .await?;
    // Channels the mover can't see (private ones) may be missing; they keep
    // their place. Everything listed must belong to this server, once.
    let known_category = |id: Option<CategoryId>| id.is_none_or(|c| categories.iter().any(|(k,)| *k == c.0));
    let mut ids: Vec<Uuid> = req.channels.iter().map(|c| c.id.0).chain(req.categories.iter().map(|c| c.0)).collect();
    let listed = ids.len();
    ids.sort();
    ids.dedup();
    if ids.len() != listed
        || !req.categories.iter().all(|c| known_category(Some(*c)))
        || !req.channels.iter().all(|c| channels.iter().any(|(k, _)| *k == c.id.0) && known_category(c.category_id))
    {
        return Err(ApiError::BadRequest("list channels and categories of this server, each once".into()));
    }
    let moved =
        req.channels.iter().any(|c| channels.iter().any(|(k, old)| *k == c.id.0 && *old != c.category_id.map(|x| x.0)));
    let mut tx = state.db.begin().await?;
    for (position, category) in req.categories.iter().enumerate() {
        sqlx::query("UPDATE categories SET position = $2 WHERE id = $1")
            .bind(category.0)
            .bind(position as i32)
            .execute(&mut *tx)
            .await?;
    }
    for (position, channel) in req.channels.iter().enumerate() {
        sqlx::query("UPDATE channels SET position = $2, category_id = $3 WHERE id = $1")
            .bind(channel.id.0)
            .bind(position as i32)
            .bind(channel.category_id.map(|c| c.0))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    if moved {
        // A channel under another category takes over its overwrites.
        access_changed(&state, guild_id).await?;
    } else {
        let guild = db::guild(&state.db, guild_id).await?;
        state.events.publish(Audience::Guild(guild_id), Event::GuildUpdate { guild });
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Moves a member to another voice channel (`MOVE_MEMBERS`): their client
/// switches the call there.
pub async fn move_member(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(guild_id): Path<GuildId>,
    Json(req): Json<MoveMemberRequest>,
) -> ApiResult<StatusCode> {
    actor(&state, guild_id, auth.id, Permissions::MOVE_MEMBERS).await?;
    let channel = db::channel(&state.db, req.channel_id).await.map_err(|_| ApiError::NotFound)?;
    if channel.guild() != Some(guild_id) || channel.kind() != ChannelKind::Voice {
        return Err(ApiError::BadRequest("move members to a voice channel of this server".into()));
    }
    // Both may have to be able to join it.
    for user in [auth.id, req.user_id] {
        match db::channel_permissions_of(&state.db, &channel, user).await? {
            Some(p) if p.allows(Permissions::VIEW_CHANNEL | Permissions::CONNECT) => {}
            Some(_) => return Err(ApiError::Forbidden),
            None => return Err(ApiError::NotFound),
        }
    }
    state.events.publish(Audience::Users(vec![req.user_id]), Event::VoiceMove { channel_id: req.channel_id });
    Ok(StatusCode::NO_CONTENT)
}
