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
pub mod presence;
mod routes;
mod storage;
mod web;

use config::Config;
use events::EventBus;

#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::PgPool,
    pub config: Arc<Config>,
    pub events: EventBus,
    pub presence: presence::Presence,
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

/// Two rustls crypto backends end up in the dependency tree (aws-lc-rs and
/// ring), so rustls can't pick one on its own and would panic on the first
/// outgoing TLS connection (e.g. to LiveKit's API). Choose aws-lc-rs.
fn install_crypto_provider() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

pub fn router(state: AppState) -> Router {
    install_crypto_provider();
    Router::new()
        .nest("/api/v1", routes::api(state.config.max_upload_bytes))
        .route("/gateway", get(gateway::handler))
        .route("/healthz", get(|| async { "ok" }))
        .route("/", get(web::home))
        .route("/logo.png", get(web::logo))
        .route("/invite/{code}", get(web::invite))
        .route("/{code}", get(web::invite))
        .layer(CompressionLayer::new())
        // Clients are native apps or served from the same origin; CORS is
        // permissive so third-party web clients can talk to any instance.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
