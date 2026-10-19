//! Sivryn server binary.

use std::sync::Arc;

use sivryn_server::config::Config;
use sivryn_server::{AppState, connect_db, event_bus, router};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sivryn_server=debug".into()))
        .init();

    let config = Config::from_env()?;
    let db = connect_db(&config.database_url).await?;
    let events = event_bus(&config).await?;

    let bind = config.bind;
    let app = router(AppState { db, config: Arc::new(config), events });

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "sivryn-server listening");
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
}
