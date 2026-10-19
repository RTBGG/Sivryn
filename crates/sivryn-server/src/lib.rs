//! Sivryn server: REST API, realtime gateway and MLS delivery service.
//!
//! The binary in `main.rs` wires configuration and signals; everything else
//! lives here so integration tests can build the same router.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::Router;
use axum::routing::get;
use sqlx::postgres::PgPoolOptions;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

mod auth;
pub mod config;
mod db;
mod error;
pub mod events;
mod gateway;
mod routes;

use config::Config;
use events::EventBus;

#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::PgPool,
    pub config: Arc<Config>,
    pub events: EventBus,
}

/// Connects to PostgreSQL and applies pending migrations.
pub async fn connect_db(url: &str) -> anyhow::Result<sqlx::PgPool> {
    let db = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(10))
        .connect(url)
        .await
        .context("connecting to PostgreSQL")?;
    migrate(&db).await?;
    Ok(db)
}

pub async fn migrate(db: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(db).await.context("running migrations")
}

/// Creates the event bus selected by the configuration.
pub async fn event_bus(config: &Config) -> anyhow::Result<EventBus> {
    match &config.nats_url {
        #[cfg(feature = "nats")]
        Some(url) => {
            tracing::info!(%url, "using NATS event bus");
            EventBus::nats(url).await.context("connecting to NATS")
        }
        #[cfg(not(feature = "nats"))]
        Some(_) => anyhow::bail!("SIVRYN_NATS_URL is set but this build has no `nats` feature"),
        None => Ok(EventBus::in_process()),
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .nest("/api/v1", routes::api())
        .route("/gateway", get(gateway::handler))
        .route("/healthz", get(|| async { "ok" }))
        .layer(CompressionLayer::new())
        // Clients are native apps or served from the same origin; CORS is
        // permissive so third-party web clients can talk to any instance.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
