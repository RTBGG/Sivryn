use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use serde::Serialize;
use sivryn_protocol::InviteId;
use sivryn_protocol::api::{InstanceInfo, InviteGuild, InvitePreview};

use crate::AppState;
use crate::auth::{AuthUser, random_token};
use crate::db;
use crate::error::{ApiError, ApiResult};

pub async fn info(State(state): State<AppState>) -> Json<InstanceInfo> {
    Json(InstanceInfo {
        name: state.config.instance_name.clone(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        registration: state.config.registration,
        oidc_enabled: state.config.oidc.is_some(),
    })
}

#[derive(Serialize)]
pub struct InviteCode {
    pub code: String,
}

/// Creates a single-use instance invite that allows registration when the
/// instance is invite-only. Admins only.
pub async fn create_invite(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<InviteCode>> {
    if !db::user(&state.db, auth.id).await?.is_admin {
        return Err(ApiError::Forbidden);
    }
    let code = random_token()[..12].to_owned();
    sqlx::query("INSERT INTO invites (id, code, created_by, max_uses) VALUES ($1, $2, $3, 1)")
        .bind(InviteId::new().0)
        .bind(&code)
        .bind(auth.id.0)
        .execute(&state.db)
        .await?;
    Ok(Json(InviteCode { code }))
}

/// What a still valid invite leads to, for link pages and the client; no sign-in needed.
pub async fn invite_preview(State(state): State<AppState>, Path(code): Path<String>) -> ApiResult<Json<InvitePreview>> {
    Ok(Json(preview(&state, &code).await?.ok_or(ApiError::NotFound)?))
}

/// The preview of `code`, or `None` when it is unknown, used up or expired.
pub(crate) async fn preview(state: &AppState, code: &str) -> ApiResult<Option<InvitePreview>> {
    let row: Option<(Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT g.name, (SELECT count(*) FROM guild_members m WHERE m.guild_id = g.id)
         FROM invites i LEFT JOIN guilds g ON g.id = i.guild_id
         WHERE i.code = $1
           AND (i.max_uses IS NULL OR i.uses < i.max_uses)
           AND (i.expires_at IS NULL OR i.expires_at > now())",
    )
    .bind(code)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|(name, members)| InvitePreview {
        instance_name: state.config.instance_name.clone(),
        guild: name.map(|name| InviteGuild { name, member_count: members.unwrap_or(0) }),
    }))
}
