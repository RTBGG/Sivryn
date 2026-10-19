//! Strongly typed identifiers.
//!
//! All IDs are UUIDv7: globally unique, generated without coordination and
//! sortable by creation time, which keeps message pagination cheap.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($($(#[$meta:meta])* $name:ident),* $(,)?) => {$(
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Creates a new, time-ordered ID.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }

        impl From<Uuid> for $name {
            fn from(id: Uuid) -> Self {
                Self(id)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }
    )*};
}

id_type!(
    /// A user account on one Sivryn instance.
    UserId,
    /// A guild ("server" in Discord terms): a community with channels and roles.
    GuildId,
    /// A text or voice channel inside a guild.
    ChannelId,
    /// A chat message.
    MessageId,
    /// A role inside a guild.
    RoleId,
    /// An invite code's internal ID.
    InviteId,
    /// A device of a user; each device has its own MLS identity.
    DeviceId,
);
