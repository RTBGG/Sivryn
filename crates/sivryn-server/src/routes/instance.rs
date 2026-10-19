use axum::Json;
use axum::extract::State;
use serde::Serialize;
use sivryn_protocol::InviteId;
use sivryn_protocol::api::InstanceInfo;

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
