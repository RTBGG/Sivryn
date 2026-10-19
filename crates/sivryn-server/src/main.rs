//! Sivryn server binary.

use std::sync::Arc;

use sivryn_server::config::Config;
use sivryn_server::{AppState, connect_db, event_bus, router};
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    // The image ships no curl or wget, so container health checks
    // call the binary itself: `sivryn-server healthcheck`.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck();
    }
    serve()
}

#[tokio::main]
async fn serve() -> anyhow::Result<()> {
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

/// Exits successfully if the local server answers `GET /healthz` with 200.
fn healthcheck() -> anyhow::Result<()> {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let bind: SocketAddr = std::env::var("SIVRYN_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into()).parse()?;
    let addr = SocketAddr::from(([127, 0, 0, 1], bind.port()));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    match response.lines().next() {
        Some(status) if status.split_whitespace().nth(1) == Some("200") => Ok(()),
        status => anyhow::bail!("unhealthy: {status:?}"),
    }
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
