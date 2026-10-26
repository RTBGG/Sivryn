use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};

use crate::AppState;

mod auth;
mod dms;
mod files;
mod friends;
mod guilds;
mod instance;

pub(crate) use instance::preview;
mod messages;
mod mls;
mod roles;
pub(crate) mod users;
pub(crate) mod voice;

/// Slack on top of the file size limit for request framing.
const UPLOAD_OVERHEAD: usize = 64 * 1024;

pub fn api(max_upload_bytes: u64) -> Router<AppState> {
    let upload_limit = DefaultBodyLimit::max(max_upload_bytes as usize + UPLOAD_OVERHEAD);
    Router::new()
        .route("/instance", get(instance::info))
        .route("/instance/invites", post(instance::create_invite))
        .route("/auth/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/oidc/start", get(auth::oidc_start))
        .route("/users/@me", get(auth::me).patch(users::update_me))
        .route("/users/@me/settings", get(users::settings))
        .route(
            "/users/@me/avatar",
            put(users::set_avatar)
                .delete(users::remove_avatar)
                .layer(DefaultBodyLimit::max(4 * 1024 * 1024 + UPLOAD_OVERHEAD)),
        )
        .route(
            "/users/@me/banner",
            put(users::set_banner)
                .delete(users::remove_banner)
                .layer(DefaultBodyLimit::max(4 * 1024 * 1024 + UPLOAD_OVERHEAD)),
        )
        .route("/users/@me/read-states", get(users::read_states))
        .route("/users/@me/mutes", get(users::mutes))
        .route("/users/{user_id}", get(users::profile))
        .route("/presence", get(users::presence))
        .route("/friends", get(friends::list))
        .route("/friends/requests", post(friends::request))
        .route("/friends/requests/{user_id}", delete(friends::cancel))
        .route("/friends/requests/{user_id}/accept", post(friends::accept))
        .route("/friends/{user_id}", delete(friends::remove))
        .route("/blocks/{user_id}", put(friends::block).delete(friends::unblock))
        .route("/dms", get(dms::list).post(dms::open))
        .route("/dms/{channel_id}", axum::routing::patch(dms::rename))
        .route("/dms/{channel_id}/members", post(dms::add_member))
        .route("/dms/{channel_id}/members/@me", delete(dms::leave))
        .route("/guilds", get(guilds::list).post(guilds::create))
        .route("/guilds/{guild_id}", get(guilds::get).patch(guilds::update).delete(guilds::delete))
        .route("/guilds/{guild_id}/categories", post(guilds::create_category))
        .route(
            "/categories/{category_id}",
            axum::routing::patch(guilds::update_category).delete(guilds::delete_category),
        )
        .route("/guilds/{guild_id}/members/@me", axum::routing::patch(guilds::update_me_member))
        .route(
            "/guilds/{guild_id}/icon",
            put(guilds::set_icon).layer(DefaultBodyLimit::max(4 * 1024 * 1024 + UPLOAD_OVERHEAD)),
        )
        .route("/guilds/{guild_id}/members", get(guilds::members))
        .route("/guilds/{guild_id}/roles", get(roles::list).post(roles::create))
        .route("/guilds/{guild_id}/roles/order", put(roles::reorder))
        .route("/roles/{role_id}", axum::routing::patch(roles::update).delete(roles::delete))
        .route(
            "/guilds/{guild_id}/members/{user_id}/roles/{role_id}",
            put(roles::add_member_role).delete(roles::remove_member_role),
        )
        .route("/channels/{channel_id}/overwrites", put(roles::set_channel_overwrite))
        .route("/channels/{channel_id}/overwrites/{kind}/{target_id}", delete(roles::delete_channel_overwrite))
        .route("/categories/{category_id}/overwrites", put(roles::set_category_overwrite))
        .route("/categories/{category_id}/overwrites/{kind}/{target_id}", delete(roles::delete_category_overwrite))
        .route("/guilds/{guild_id}/layout", put(roles::layout))
        .route("/guilds/{guild_id}/voice/move", post(roles::move_member))
        .route("/guilds/{guild_id}/members/{user_id}", delete(guilds::remove_member))
        .route("/guilds/{guild_id}/bans", get(guilds::bans))
        .route("/guilds/{guild_id}/bans/{user_id}", put(guilds::ban).delete(guilds::unban))
        .route("/guilds/{guild_id}/voice", get(voice::guild_states))
        .route("/guilds/{guild_id}/channels", post(guilds::create_channel))
        .route("/guilds/{guild_id}/invites", post(guilds::create_invite))
        .route("/invites/{code}", get(instance::invite_preview).post(guilds::accept_invite))
        .route("/channels/{channel_id}", axum::routing::patch(guilds::update_channel).delete(guilds::delete_channel))
        .route("/channels/{channel_id}/read", put(users::ack))
        .route("/channels/{channel_id}/mute", put(users::mute).delete(users::unmute))
        .route("/channels/{channel_id}/members", get(guilds::channel_members))
        .route("/channels/{channel_id}/messages", get(messages::list).post(messages::send))
        .route("/channels/{channel_id}/messages/{message_id}", delete(messages::delete))
        .route("/channels/{channel_id}/attachments", post(files::upload).layer(upload_limit))
        .route("/channels/{channel_id}/mls/commits", get(mls::commits_since).post(mls::commit))
        .route("/channels/{channel_id}/voice", get(voice::channel_state))
        .route("/channels/{channel_id}/voice/join", post(voice::join))
        .route("/channels/{channel_id}/voice/{user_id}", delete(voice::disconnect))
        .route("/files/{file_id}", get(files::download))
        .route("/devices", post(mls::register_device))
        .route("/devices/{device_id}/key-packages", get(mls::count_key_packages).post(mls::upload_key_packages))
        .route("/devices/{device_id}/welcomes", get(mls::take_welcomes))
        .route("/users/{user_id}/key-packages/claim", post(mls::claim_key_packages))
}
