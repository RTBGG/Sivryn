//! History keys: reading a channel's earlier messages after joining it.
//!
//! MLS deletes every message key after use, so a member added later can't
//! decrypt anything sent before. In guilds that share history, every message
//! is therefore also sealed with the channel's history key (XChaCha20-
//! Poly1305) and stored on the server next to the MLS ciphertext. The keys
//! themselves only travel inside MLS: whoever adds someone to a channel's
//! group hands them all keys, and removing someone starts a new key, so they
//! can't read what comes after.
//!
//! The trade: whoever gets hold of a history key can read the history sealed
//! with it, where plain MLS would expose only a few messages.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use sivryn_client_core::protocol::model::{HistoryKey, MessageContent, MessageKind};
use sivryn_client_core::protocol::{ChannelId, MessageId, UserId};

const ID_LEN: usize = 16;
const NONCE_LEN: usize = 24;

/// A new random key.
pub fn new_key() -> HistoryKey {
    let mut id = [0u8; ID_LEN];
    let mut key = [0u8; 32];
    getrandom::fill(&mut id).expect("OS random number generator");
    getrandom::fill(&mut key).expect("OS random number generator");
    HistoryKey { id: B64.encode(id), key: B64.encode(key), created_at: chrono::Utc::now().timestamp_millis() }
}

/// The key that seals new messages: the newest one.
pub fn current(keys: &[HistoryKey]) -> Option<&HistoryKey> {
    keys.iter().max_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)))
}

/// What a sealed copy is bound to, so the server can't move it to another
/// channel, author or message it points at.
fn context(channel: ChannelId, author: UserId, kind: MessageKind, target: Option<MessageId>) -> String {
    let target = target.map(|t| t.to_string()).unwrap_or_default();
    format!("sivryn history v1|{channel}|{author}|{}|{target}", kind.as_str())
}

/// Seals `content` with `key`: key id, nonce and ciphertext, base64.
pub fn seal(
    key: &HistoryKey,
    channel: ChannelId,
    author: UserId,
    kind: MessageKind,
    target: Option<MessageId>,
    content: &MessageContent,
) -> Option<String> {
    let id = B64.decode(&key.id).ok().filter(|id| id.len() == ID_LEN)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&B64.decode(&key.key).ok()?).ok()?;
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).expect("OS random number generator");
    let plain = serde_json::to_vec(content).ok()?;
    let aad = context(channel, author, kind, target);
    let sealed = cipher.encrypt(&XNonce::from(nonce), Payload { msg: &plain, aad: aad.as_bytes() }).ok()?;
    Some(B64.encode([id.as_slice(), &nonce, &sealed].concat()))
}

/// Opens a sealed copy with whichever of `keys` it names.
pub fn open(
    keys: &[HistoryKey],
    sealed: &str,
    channel: ChannelId,
    author: UserId,
    kind: MessageKind,
    target: Option<MessageId>,
) -> Option<MessageContent> {
    let bytes = B64.decode(sealed).ok()?;
    if bytes.len() < ID_LEN + NONCE_LEN {
        return None;
    }
    let (id, rest) = bytes.split_at(ID_LEN);
    let (nonce, ciphertext) = rest.split_at(NONCE_LEN);
    let key = keys.iter().find(|k| B64.decode(&k.id).is_ok_and(|kid| kid == id))?;
    let cipher = XChaCha20Poly1305::new_from_slice(&B64.decode(&key.key).ok()?).ok()?;
    let aad = context(channel, author, kind, target);
    let nonce = XNonce::try_from(nonce).ok()?;
    let plain = cipher.decrypt(&nonce, Payload { msg: ciphertext, aad: aad.as_bytes() }).ok()?;
    serde_json::from_slice(&plain).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_copies_open_with_their_key_and_context_only() {
        let (channel, author) = (ChannelId::new(), UserId::new());
        let old = new_key();
        let mut new = new_key();
        new.created_at = old.created_at + 1;
        let keys = vec![old.clone(), new.clone()];
        assert_eq!(current(&keys), Some(&new));

        let content = MessageContent { body: "hello later".into(), ..Default::default() };
        let sealed = seal(&old, channel, author, MessageKind::Message, None, &content).unwrap();
        assert_eq!(open(&keys, &sealed, channel, author, MessageKind::Message, None), Some(content.clone()));
        // Without the key, or moved to another channel or author: nothing.
        assert_eq!(open(std::slice::from_ref(&new), &sealed, channel, author, MessageKind::Message, None), None);
        assert_eq!(open(&keys, &sealed, ChannelId::new(), author, MessageKind::Message, None), None);
        assert_eq!(open(&keys, &sealed, channel, UserId::new(), MessageKind::Message, None), None);
        assert_eq!(open(&keys, &sealed, channel, author, MessageKind::Edit, Some(MessageId::new())), None);
        assert_eq!(open(&keys, "garbage", channel, author, MessageKind::Message, None), None);
    }
}
