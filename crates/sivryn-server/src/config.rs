//! Server configuration, read from environment variables (see `deploy/.env.example`).

use std::env;
use std::net::SocketAddr;

use anyhow::{Context, bail};
use sivryn_protocol::api::RegistrationMode;
use sivryn_protocol::media::{CameraLimits, ScreenShareLimits, VideoCodec};

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub instance_name: String,
    pub database_url: String,
    pub registration: RegistrationMode,
    pub session_ttl_days: i64,
    pub livekit: LiveKitConfig,
    pub screen_share: ScreenShareLimits,
    pub camera: CameraLimits,
    /// When set, gateway events are fanned out through NATS so several server
    /// instances can run behind a load balancer.
    pub nats_url: Option<String>,
    pub oidc: Option<OidcConfig>,
}

#[derive(Clone, Debug)]
pub struct LiveKitConfig {
    /// URL handed to clients, e.g. `wss://chat.example.com`.
    pub public_url: String,
    pub api_key: String,
    pub api_secret: String,
}

#[derive(Clone, Debug)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let registration = match var_or("SIVRYN_REGISTRATION", "invite_only").as_str() {
            "open" => RegistrationMode::Open,
            "invite_only" => RegistrationMode::InviteOnly,
            "closed" => RegistrationMode::Closed,
            other => bail!("SIVRYN_REGISTRATION must be open, invite_only or closed, got {other:?}"),
        };

        let defaults = ScreenShareLimits::default();
        let screen_share = ScreenShareLimits {
            max_width: parse_or("SIVRYN_SCREENSHARE_MAX_WIDTH", defaults.max_width)?,
            max_height: parse_or("SIVRYN_SCREENSHARE_MAX_HEIGHT", defaults.max_height)?,
            max_fps: parse_or("SIVRYN_SCREENSHARE_MAX_FPS", defaults.max_fps)?,
            max_bitrate_bps: parse_or("SIVRYN_SCREENSHARE_MAX_BITRATE", defaults.max_bitrate_bps)?,
            codec_preference: match env::var("SIVRYN_SCREENSHARE_CODECS") {
                Ok(list) => parse_codecs(&list)?,
                Err(_) => defaults.codec_preference,
            },
        };

        let oidc = match env::var("SIVRYN_OIDC_ISSUER") {
            Ok(issuer_url) => Some(OidcConfig {
                issuer_url,
                client_id: required("SIVRYN_OIDC_CLIENT_ID")?,
                client_secret: required("SIVRYN_OIDC_CLIENT_SECRET")?,
            }),
            Err(_) => None,
        };

        Ok(Self {
            bind: var_or("SIVRYN_BIND", "0.0.0.0:8080").parse().context("SIVRYN_BIND is not a valid socket address")?,
            instance_name: var_or("SIVRYN_INSTANCE_NAME", "Sivryn"),
            database_url: required("SIVRYN_DATABASE_URL")?,
            registration,
            session_ttl_days: parse_or("SIVRYN_SESSION_TTL_DAYS", 30)?,
            livekit: LiveKitConfig {
                public_url: required("SIVRYN_LIVEKIT_URL")?,
                api_key: required("LIVEKIT_API_KEY")?,
                api_secret: required("LIVEKIT_API_SECRET")?,
            },
            screen_share,
            camera: CameraLimits::default(),
            nats_url: env::var("SIVRYN_NATS_URL").ok().filter(|s| !s.is_empty()),
            oidc,
        })
    }
}

fn required(name: &str) -> anyhow::Result<String> {
    env::var(name).with_context(|| format!("environment variable {name} is required"))
}

fn var_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn parse_or<T: std::str::FromStr>(name: &str, default: T) -> anyhow::Result<T>
where
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(v) => v.parse().map_err(|e| anyhow::anyhow!("{name}: {e}")),
        Err(_) => Ok(default),
    }
}

fn parse_codecs(list: &str) -> anyhow::Result<Vec<VideoCodec>> {
    list.split(',')
        .map(|c| match c.trim().to_ascii_lowercase().as_str() {
            "av1" => Ok(VideoCodec::Av1),
            "h264" => Ok(VideoCodec::H264),
            "vp9" => Ok(VideoCodec::Vp9),
            "vp8" => Ok(VideoCodec::Vp8),
            other => bail!("unknown codec {other:?} in SIVRYN_SCREENSHARE_CODECS"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codec_list() {
        assert_eq!(parse_codecs("AV1, h264").unwrap(), vec![VideoCodec::Av1, VideoCodec::H264]);
        assert!(parse_codecs("hevc").is_err());
    }
}
