//! Who is online: open gateway connections per user on this instance.
//! (With several instances behind NATS each instance counts its own
//! connections, so a user connected to two instances can flicker offline.)

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sivryn_protocol::UserId;

#[derive(Clone, Default)]
pub struct Presence {
    connections: Arc<Mutex<HashMap<UserId, usize>>>,
}

impl Presence {
    /// Counts a new connection; `true` if the user just came online.
    pub fn connect(&self, user: UserId) -> bool {
        let mut map = self.connections.lock().unwrap_or_else(|e| e.into_inner());
        let count = map.entry(user).or_default();
        *count += 1;
        *count == 1
    }

    /// Counts a closed connection; `true` if the user just went offline.
    pub fn disconnect(&self, user: UserId) -> bool {
        let mut map = self.connections.lock().unwrap_or_else(|e| e.into_inner());
        match map.get_mut(&user) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => {
                map.remove(&user);
                true
            }
            None => false,
        }
    }

    pub fn is_online(&self, user: UserId) -> bool {
        self.connections.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_connection_keeps_user_online() {
        let p = Presence::default();
        let u = UserId::new();
        assert!(p.connect(u));
        assert!(!p.connect(u));
        assert!(!p.disconnect(u));
        assert!(p.is_online(u));
        assert!(p.disconnect(u));
        assert!(!p.is_online(u));
        assert!(!p.disconnect(u));
    }
}
