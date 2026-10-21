use bitflags::bitflags;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::model::{OverwriteTarget, PermissionOverwrite};
use crate::{RoleId, UserId};

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

/// A member's permissions in one channel, Discord style: start from the
/// guild permissions of all their roles, then apply each layer of overwrites
/// (the category's, then the channel's). Within a layer, `@everyone` comes
/// first, then all of the member's roles together (denies, then allows), then
/// the member's own overwrite. Administrators keep everything.
pub fn channel_permissions(
    guild: Permissions,
    everyone: RoleId,
    roles: &[RoleId],
    user: UserId,
    layers: &[&[PermissionOverwrite]],
) -> Permissions {
    if guild.contains(Permissions::ADMINISTRATOR) {
        return Permissions::all();
    }
    let mut p = guild;
    for layer in layers {
        let find = |target: OverwriteTarget| layer.iter().find(|o| o.target == target);
        if let Some(o) = find(OverwriteTarget::Role(everyone)) {
            p = (p - o.deny) | o.allow;
        }
        let (mut deny, mut allow) = (Permissions::empty(), Permissions::empty());
        for role in roles {
            if let Some(o) = find(OverwriteTarget::Role(*role)) {
                deny |= o.deny;
                allow |= o.allow;
            }
        }
        p = (p - deny) | allow;
        if let Some(o) = find(OverwriteTarget::Member(user)) {
            p = (p - o.deny) | o.allow;
        }
    }
    p
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

    fn overwrite(target: OverwriteTarget, allow: Permissions, deny: Permissions) -> PermissionOverwrite {
        PermissionOverwrite { target, allow, deny }
    }

    #[test]
    fn channel_overwrites_apply_in_discord_order() {
        let (everyone, mods, user) = (RoleId::new(), RoleId::new(), UserId::new());
        let base = Permissions::default_member();
        // A private category: hidden from everyone, visible to moderators.
        let category = [
            overwrite(OverwriteTarget::Role(everyone), Permissions::empty(), Permissions::VIEW_CHANNEL),
            overwrite(OverwriteTarget::Role(mods), Permissions::VIEW_CHANNEL, Permissions::empty()),
        ];
        let hidden = channel_permissions(base, everyone, &[], user, &[&category]);
        assert!(!hidden.contains(Permissions::VIEW_CHANNEL));
        let visible = channel_permissions(base, everyone, &[mods], user, &[&category]);
        assert!(visible.contains(Permissions::VIEW_CHANNEL));

        // The channel can take writing away again, and a member overwrite wins.
        let channel = [overwrite(OverwriteTarget::Role(mods), Permissions::empty(), Permissions::SEND_MESSAGES)];
        let read_only = channel_permissions(base, everyone, &[mods], user, &[&category, &channel]);
        assert!(read_only.contains(Permissions::VIEW_CHANNEL) && !read_only.contains(Permissions::SEND_MESSAGES));
        let member = [overwrite(OverwriteTarget::Member(user), Permissions::SEND_MESSAGES, Permissions::empty())];
        let p = channel_permissions(base, everyone, &[mods], user, &[&category, &channel, &member]);
        assert!(p.contains(Permissions::SEND_MESSAGES));

        // Administrators ignore overwrites.
        let admin = channel_permissions(Permissions::ADMINISTRATOR, everyone, &[], user, &[&category]);
        assert_eq!(admin, Permissions::all());
    }

    #[test]
    fn serializes_as_number() {
        let json = serde_json::to_string(&Permissions::VIEW_CHANNEL).unwrap();
        assert_eq!(json, (1u64 << 10).to_string());
    }
}
