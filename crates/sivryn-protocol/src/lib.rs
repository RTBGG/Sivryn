//! Wire types shared by the Sivryn server and all clients.
//!
//! Everything in here is serialized as JSON: over HTTP for the REST API and
//! over WebSocket for the realtime gateway. Media (voice, video, screen share)
//! does not travel through these types; it flows through the LiveKit SFU.

pub mod api;
pub mod gateway;
pub mod ids;
pub mod media;
pub mod model;
pub mod permissions;

pub use ids::*;

/// Version of the gateway protocol. Bumped on breaking changes.
pub const GATEWAY_VERSION: u32 = 2;
