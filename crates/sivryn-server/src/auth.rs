//! Password hashing, session tokens and the [`AuthUser`] extractor.

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use rand::RngExt;
use sha2::{Digest, Sha256};
use sivryn_protocol::UserId;
use sqlx::PgPool;

use crate::AppState;
use crate::error::{ApiError, ApiResult};

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let hash =
        Argon2::default().hash_password(password.as_bytes()).map_err(|e| anyhow::anyhow!("hashing password: {e}"))?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
}

/// Generates a random URL-safe token with 256 bits of entropy.
pub fn random_token() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    URL_SAFE_NO_PAD.encode(bytes)
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

pub async fn create_session(pool: &PgPool, user_id: UserId, ttl_days: i64) -> ApiResult<String> {
    let token = random_token();
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(token_hash(&token))
        .bind(user_id.0)
        .bind(Utc::now() + Duration::days(ttl_days))
        .execute(pool)
        .await?;
    Ok(token)
}

pub async fn delete_session(pool: &PgPool, token: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = $1").bind(token_hash(token)).execute(pool).await?;
    Ok(())
}

/// Resolves a bearer token to its user, if the session exists and is not expired.
pub async fn user_for_token(pool: &PgPool, token: &str) -> ApiResult<Option<UserId>> {
    let row: Option<(uuid::Uuid,)> =
        sqlx::query_as("SELECT user_id FROM sessions WHERE token_hash = $1 AND expires_at > now()")
            .bind(token_hash(token))
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(id,)| UserId(id)))
}

/// An authenticated request. Extracting it rejects requests without a valid
/// `Authorization: Bearer <token>` header.
pub struct AuthUser {
    pub id: UserId,
    pub token: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(ApiError::Unauthorized)?
            .to_owned();
        let id = user_for_token(&state.db, &token).await?.ok_or(ApiError::Unauthorized)?;
        Ok(Self { id, token })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_round_trip() {
        let hash = hash_password("correct horse").unwrap();
        assert!(verify_password("correct horse", &hash));
        assert!(!verify_password("battery staple", &hash));
    }

    #[test]
    fn tokens_are_unique_and_url_safe() {
        let a = random_token();
        assert_ne!(a, random_token());
        assert_eq!(a.len(), 43);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }
}
