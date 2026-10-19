//! MLS delivery service.
//!
//! The server never sees group secrets. It stores one-time key packages,
//! orders commits per channel (one per epoch) and relays welcomes to the
//! devices they are addressed to.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use sivryn_protocol::api::{
    ClaimedKeyPackage, KeyPackageCount, MlsCommitRequest, MlsWelcome, PendingWelcome, RegisterDeviceRequest,
    RegisterDeviceResponse, StoredCommit,
};
use sivryn_protocol::gateway::Event;
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{ChannelId, DeviceId, GuildId, UserId};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::db;
use crate::error::{ApiError, ApiResult};

const MAX_KEY_PACKAGES_PER_UPLOAD: usize = 100;

pub async fn register_device(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<RegisterDeviceRequest>,
) -> ApiResult<(StatusCode, Json<RegisterDeviceResponse>)> {
    let device_id = DeviceId::new();
    let mut tx = state.db.begin().await?;
    sqlx::query("INSERT INTO devices (id, user_id, name) VALUES ($1, $2, $3)")
        .bind(device_id.0)
        .bind(auth.id.0)
        .bind(req.name.trim())
        .execute(&mut *tx)
        .await?;
    insert_key_packages(&mut tx, device_id, &req.key_packages).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(RegisterDeviceResponse { device_id })))
}

#[derive(Deserialize)]
pub struct KeyPackageUpload {
    pub key_packages: Vec<String>,
}

pub async fn count_key_packages(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(device_id): Path<DeviceId>,
) -> ApiResult<Json<KeyPackageCount>> {
    db::require_own_device(&state.db, device_id, auth.id).await?;
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM key_packages WHERE device_id = $1")
        .bind(device_id.0)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(KeyPackageCount { count: count as u32 }))
}

pub async fn upload_key_packages(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(device_id): Path<DeviceId>,
    Json(req): Json<KeyPackageUpload>,
) -> ApiResult<StatusCode> {
    db::require_own_device(&state.db, device_id, auth.id).await?;
    let mut tx = state.db.begin().await?;
    insert_key_packages(&mut tx, device_id, &req.key_packages).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn insert_key_packages(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    device_id: DeviceId,
    packages: &[String],
) -> ApiResult<()> {
    if packages.len() > MAX_KEY_PACKAGES_PER_UPLOAD {
        return Err(ApiError::BadRequest("too many key packages".into()));
    }
    for kp in packages {
        sqlx::query("INSERT INTO key_packages (device_id, key_package) VALUES ($1, $2)")
            .bind(device_id.0)
            .bind(kp)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Claims (and deletes) one key package for each of the user's devices, so the
/// caller can add all of them to an MLS group.
pub async fn claim_key_packages(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<UserId>,
) -> ApiResult<Json<Vec<ClaimedKeyPackage>>> {
    // Only people who share a guild may add someone to a group; otherwise
    // anyone could drain a user's one-time key packages.
    let (shares_guild,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (
             SELECT 1 FROM guild_members a JOIN guild_members b ON a.guild_id = b.guild_id
             WHERE a.user_id = $1 AND b.user_id = $2
         )",
    )
    .bind(auth.id.0)
    .bind(user_id.0)
    .fetch_one(&state.db)
    .await?;
    if !shares_guild {
        return Err(ApiError::NotFound);
    }

    // Oldest package per device. Two concurrent claims may pick the same row;
    // the second DELETE then finds it gone and simply returns fewer packages.
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "DELETE FROM key_packages k
         USING (
             SELECT min(kp.id) AS id
             FROM key_packages kp JOIN devices d ON d.id = kp.device_id
             WHERE d.user_id = $1
             GROUP BY kp.device_id
         ) oldest
         WHERE k.id = oldest.id
         RETURNING k.device_id, k.key_package",
    )
    .bind(user_id.0)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|(device_id, key_package)| ClaimedKeyPackage { user_id, device_id: DeviceId(device_id), key_package })
            .collect(),
    ))
}

pub async fn commit(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
    Json(req): Json<MlsCommitRequest>,
) -> ApiResult<StatusCode> {
    let channel = db::require_channel(&state.db, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    db::require_own_device(&state.db, req.device_id, auth.id).await?;

    let mut tx = state.db.begin().await?;
    sqlx::query("INSERT INTO mls_commits (channel_id, epoch, device_id, commit) VALUES ($1, $2, $3, $4)")
        .bind(channel_id.0)
        .bind(req.epoch as i64)
        .bind(req.device_id.0)
        .bind(&req.commit)
        .execute(&mut *tx)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                ApiError::Conflict("another commit won this epoch; fetch commits and retry".into())
            }
            _ => e.into(),
        })?;

    if let Some(welcome) = &req.welcome {
        for device in &welcome.recipients {
            sqlx::query(
                "INSERT INTO pending_welcomes (device_id, channel_id, welcome, ratchet_tree) VALUES ($1, $2, $3, $4)",
            )
            .bind(device.0)
            .bind(channel_id.0)
            .bind(&welcome.welcome)
            .bind(&welcome.ratchet_tree)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;

    state.events.publish(Event::MlsCommit {
        guild_id: GuildId(channel.guild_id),
        channel_id,
        epoch: req.epoch,
        commit: req.commit,
    });
    if let Some(welcome) = req.welcome {
        state.events.publish(Event::MlsWelcome { channel_id, welcome });
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct CommitsSince {
    pub since: u64,
}

/// Commits created in epoch `since` or later, for devices catching up from that epoch.
pub async fn commits_since(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<ChannelId>,
    Query(q): Query<CommitsSince>,
) -> ApiResult<Json<Vec<StoredCommit>>> {
    db::require_channel(&state.db, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT epoch, commit FROM mls_commits WHERE channel_id = $1 AND epoch >= $2 ORDER BY epoch")
            .bind(channel_id.0)
            .bind(q.since as i64)
            .fetch_all(&state.db)
            .await?;
    Ok(Json(rows.into_iter().map(|(epoch, commit)| StoredCommit { epoch: epoch as u64, commit }).collect()))
}

/// Returns and deletes welcomes waiting for one of the caller's devices.
pub async fn take_welcomes(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(device_id): Path<DeviceId>,
) -> ApiResult<Json<Vec<PendingWelcome>>> {
    db::require_own_device(&state.db, device_id, auth.id).await?;
    let rows: Vec<(Uuid, String, String)> =
        sqlx::query_as("DELETE FROM pending_welcomes WHERE device_id = $1 RETURNING channel_id, welcome, ratchet_tree")
            .bind(device_id.0)
            .fetch_all(&state.db)
            .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(channel_id, welcome, ratchet_tree)| PendingWelcome {
                channel_id: ChannelId(channel_id),
                welcome: MlsWelcome { recipients: vec![device_id], welcome, ratchet_tree },
            })
            .collect(),
    ))
}
