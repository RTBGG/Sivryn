use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use sivryn_protocol::api::{LoginRequest, RegisterRequest, RegistrationMode, SessionResponse};
use sivryn_protocol::gateway::{Audience, Event};
use sivryn_protocol::model::User;
use sivryn_protocol::{GuildId, UserId};
use uuid::Uuid;

use crate::AppState;
use crate::auth::{self, AuthUser};
use crate::db::{self, UserRow};
use crate::error::{ApiError, ApiResult};

const MIN_PASSWORD_LEN: usize = 10;

fn validate_username(name: &str) -> ApiResult<()> {
    let ok = (2..=32).contains(&name.chars().count())
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok {
        Ok(())
    } else {
        Err(ApiError::BadRequest("username must be 2-32 characters: letters, digits, _ - .".into()))
    }
}

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> ApiResult<(StatusCode, Json<SessionResponse>)> {
    validate_username(&req.username)?;
    if req.password.chars().count() < MIN_PASSWORD_LEN {
        return Err(ApiError::BadRequest(format!("password must be at least {MIN_PASSWORD_LEN} characters")));
    }

    let mut tx = state.db.begin().await?;

    let (user_count,): (i64,) = sqlx::query_as("SELECT count(*) FROM users").fetch_one(&mut *tx).await?;
    // The very first account bootstraps the instance and becomes admin,
    // regardless of the registration mode.
    let is_first = user_count == 0;

    // An instance invite or a server invite: either lets someone register on
    // an invite-only instance; a server invite also makes them a member.
    let code = req.invite_code.as_deref().map(str::trim).filter(|c| !c.is_empty());
    let mut join_guild = None;
    if !is_first {
        match (state.config.registration, code) {
            (RegistrationMode::Closed, _) => return Err(ApiError::RegistrationClosed),
            (RegistrationMode::InviteOnly, None) => return Err(ApiError::InviteRequired),
            (_, None) => {}
            (_, Some(code)) => {
                let instance = sqlx::query(
                    "UPDATE invites SET uses = uses + 1
                     WHERE code = $1 AND guild_id IS NULL
                       AND (max_uses IS NULL OR uses < max_uses)
                       AND (expires_at IS NULL OR expires_at > now())",
                )
                .bind(code)
                .execute(&mut *tx)
                .await?;
                if instance.rows_affected() == 0 {
                    let guild: Option<(Uuid,)> = sqlx::query_as(
                        "UPDATE invites SET uses = uses + 1
                         WHERE code = $1 AND guild_id IS NOT NULL
                           AND (max_uses IS NULL OR uses < max_uses)
                           AND (expires_at IS NULL OR expires_at > now())
                         RETURNING guild_id",
                    )
                    .bind(code)
                    .fetch_optional(&mut *tx)
                    .await?;
                    join_guild = Some(GuildId(guild.ok_or(ApiError::InvalidInvite)?.0));
                }
            }
        }
    }

    let hash = auth::hash_password(&req.password)?;
    let user: UserRow =
        sqlx::query_as("INSERT INTO users (id, username, password_hash, is_admin) VALUES ($1, $2, $3, $4) RETURNING *")
            .bind(Uuid::now_v7())
            .bind(&req.username)
            .bind(hash)
            .bind(is_first)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(db) if db.is_unique_violation() => {
                    ApiError::Conflict("username is already taken".into())
                }
                _ => e.into(),
            })?;
    if let Some(guild_id) = join_guild {
        sqlx::query("INSERT INTO guild_members (guild_id, user_id) VALUES ($1, $2)")
            .bind(guild_id.0)
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    let id = UserId(user.id);
    let user: User = user.into();
    if let Some(guild_id) = join_guild {
        state.events.publish(Audience::Guild(guild_id), Event::MemberJoin { guild_id, user: user.clone() });
    }

    let token = auth::create_session(&state.db, id, state.config.session_ttl_days).await?;
    Ok((StatusCode::CREATED, Json(SessionResponse { token, user })))
}

pub async fn login(State(state): State<AppState>, Json(req): Json<LoginRequest>) -> ApiResult<Json<SessionResponse>> {
    let user: Option<UserRow> = sqlx::query_as("SELECT * FROM users WHERE lower(username) = lower($1)")
        .bind(&req.username)
        .fetch_optional(&state.db)
        .await?;

    let user = user
        .filter(|u| u.password_hash.as_deref().is_some_and(|h| auth::verify_password(&req.password, h)))
        .ok_or(ApiError::Unauthorized)?;

    let token = auth::create_session(&state.db, UserId(user.id), state.config.session_ttl_days).await?;
    Ok(Json(SessionResponse { token, user: user.into() }))
}

pub async fn logout(State(state): State<AppState>, auth: AuthUser) -> ApiResult<StatusCode> {
    auth::delete_session(&state.db, &auth.token).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn me(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<User>> {
    Ok(Json(db::user(&state.db, auth.id).await?.into()))
}

/// OIDC login (Authentik, Keycloak, ...). Configuration is already read from
/// `SIVRYN_OIDC_*`; the authorization-code flow itself is not implemented yet.
pub async fn oidc_start(State(state): State<AppState>) -> ApiResult<()> {
    if state.config.oidc.is_none() {
        return Err(ApiError::NotFound);
    }
    Err(ApiError::NotImplemented("OIDC login is not implemented yet"))
}
