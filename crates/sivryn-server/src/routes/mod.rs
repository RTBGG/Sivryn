use axum::Router;
use axum::routing::{get, post};

use crate::AppState;

mod auth;
mod guilds;
mod instance;
mod messages;
mod mls;
mod voice;

pub fn api() -> Router<AppState> {
    Router::new()
        .route("/instance", get(instance::info))
        .route("/instance/invites", post(instance::create_invite))
        .route("/auth/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/oidc/start", get(auth::oidc_start))
        .route("/users/@me", get(auth::me))
        .route("/guilds", get(guilds::list).post(guilds::create))
        .route("/guilds/{guild_id}", get(guilds::get))
        .route("/guilds/{guild_id}/members", get(guilds::members))
        .route("/guilds/{guild_id}/voice", get(voice::guild_states))
        .route("/guilds/{guild_id}/channels", post(guilds::create_channel))
        .route("/guilds/{guild_id}/invites", post(guilds::create_invite))
        .route("/invites/{code}", post(guilds::accept_invite))
        .route("/channels/{channel_id}/messages", get(messages::list).post(messages::send))
        .route("/channels/{channel_id}/mls/commits", get(mls::commits_since).post(mls::commit))
        .route("/channels/{channel_id}/voice/join", post(voice::join))
        .route("/devices", post(mls::register_device))
        .route("/devices/{device_id}/key-packages", get(mls::count_key_packages).post(mls::upload_key_packages))
        .route("/devices/{device_id}/welcomes", get(mls::take_welcomes))
        .route("/users/{user_id}/key-packages/claim", post(mls::claim_key_packages))
}
