//! The desktop build links rustls with both `ring` and `aws-lc-rs` (via
//! reqwest, tokio-tungstenite and LiveKit). Without an explicit provider,
//! the first `wss://` connection panicked right after login.

#[test]
fn tls_client_config_builds_after_installing_provider() {
    sivryn_client_core::install_crypto_provider();
    let _ = rustls::ClientConfig::builder();
}
