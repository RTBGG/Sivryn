use bitflags::bitflags;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

bitflags! {
    /// Guild permissions, stored as a 64-bit mask per role.
    /// On the wire this is a plain number, so clients in any language can use it.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Permissions: u64 {
        const ADMINISTRATOR    = 1 << 0;
        const MANAGE_GUILD     = 1 << 1;
        const MANAGE_CHANNELS  = 1 << 2;
        const MANAGE_ROLES     = 1 << 3;
        const CREATE_INVITE    = 1 << 4;
        const KICK_MEMBERS     = 1 << 5;
        const BAN_MEMBERS      = 1 << 6;
        const VIEW_CHANNEL     = 1 << 10;
        const SEND_MESSAGES    = 1 << 11;
        const MANAGE_MESSAGES  = 1 << 12;
        const ATTACH_FILES     = 1 << 13;
        const CONNECT          = 1 << 20;
        const SPEAK            = 1 << 21;
        const VIDEO            = 1 << 22;
        const SCREEN_SHARE     = 1 << 23;
        const MUTE_MEMBERS     = 1 << 24;
        const MOVE_MEMBERS     = 1 << 25;
    }
}

impl Permissions {
    /// Permissions of the implicit `@everyone` role in a new guild.
    pub fn default_member() -> Self {
        Self::VIEW_CHANNEL
            | Self::SEND_MESSAGES
            | Self::ATTACH_FILES
            | Self::CREATE_INVITE
            | Self::CONNECT
            | Self::SPEAK
            | Self::VIDEO
            | Self::SCREEN_SHARE
    }

    /// Whether `self` grants `required`, honouring `ADMINISTRATOR`.
    pub fn allows(self, required: Permissions) -> bool {
        self.contains(Self::ADMINISTRATOR) || self.contains(required)
    }
}

impl Serialize for Permissions {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.bits())
    }
}

impl<'de> Deserialize<'de> for Permissions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Unknown bits from newer servers are dropped rather than rejected.
        u64::deserialize(deserializer).map(Self::from_bits_truncate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn administrator_allows_everything() {
        assert!(Permissions::ADMINISTRATOR.allows(Permissions::BAN_MEMBERS));
    }

    #[test]
    fn default_member_can_screen_share_but_not_manage() {
        let p = Permissions::default_member();
        assert!(p.allows(Permissions::SCREEN_SHARE));
        assert!(!p.allows(Permissions::MANAGE_CHANNELS));
    }

    #[test]
    fn serializes_as_number() {
        let json = serde_json::to_string(&Permissions::VIEW_CHANNEL).unwrap();
        assert_eq!(json, (1u64 << 10).to_string());
    }
}
