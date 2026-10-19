//! Local, encrypted account storage.
//!
//! Each account (instance + user) gets one SQLite database in
//! `%APPDATA%\Sivryn\accounts\`. It holds the MLS device state and the
//! decrypted message history: MLS keys are single-use, so a message read once
//! can never be decrypted from the server's ciphertext again.
//!
//! Secret columns are sealed with XChaCha20-Poly1305 under a random 256-bit
//! key that lives, together with the session token, in the OS credential
//! store (Windows Credential Manager). The row's identity is bound in as
//! associated data, so sealed values can't be swapped between rows. Channel
//! and author IDs and timestamps stay readable for indexing.

use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sivryn_client_core::protocol::{ChannelId, DeviceId, MessageId, UserId};

use crate::chat::ChatLine;

const KEYRING_SERVICE: &str = "Sivryn";
const NONCE_LEN: usize = 24;

/// What lives in the OS credential store for one account.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    /// Session token; `None` after logging out (history stays readable).
    pub token: Option<String>,
    /// Base64 key for the account database.
    db_key: String,
}

/// Identifies an account: the instance URL and the user's ID on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountId {
    pub instance: String,
    pub user_id: UserId,
}

impl AccountId {
    fn keyring_user(&self) -> String {
        format!("{}|{}", self.instance, self.user_id)
    }

    fn file_name(&self) -> String {
        // Instance URLs contain characters that aren't valid in file names.
        let host: String = self
            .instance
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
            .collect();
        format!("{host}-{}.db", self.user_id)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("credential store: {0}")]
    Keyring(#[from] keyring::Error),
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("local data is corrupt or was encrypted with another key")]
    Corrupt,
    #[error("no config directory")]
    NoDir,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

fn entry(account: &AccountId) -> Result<keyring::Entry, StoreError> {
    Ok(keyring::Entry::new(KEYRING_SERVICE, &account.keyring_user())?)
}

/// Reads an account's credentials, `None` if there are none.
pub fn load_credentials(account: &AccountId) -> Result<Option<Credentials>, StoreError> {
    match entry(account)?.get_secret() {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn save_credentials(account: &AccountId, creds: &Credentials) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(creds).expect("credentials serialize");
    Ok(entry(account)?.set_secret(&bytes)?)
}

pub fn accounts_dir() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("accounts"))
}

/// One account's open database.
pub struct Store {
    db: Connection,
    cipher: XChaCha20Poly1305,
    account: AccountId,
}

impl Store {
    /// Opens (or creates) the account's database and remembers `token`.
    pub fn open(account: &AccountId, token: Option<&str>) -> Result<Self, StoreError> {
        let mut creds = load_credentials(account)?;
        let path = accounts_dir().ok_or(StoreError::NoDir)?.join(account.file_name());

        // A database without its key is unreadable: start over rather than fail.
        if creds.is_none() && path.exists() {
            tracing::warn!(path = %path.display(), "account database has no key; replacing it");
            std::fs::remove_file(&path)?;
        }
        let mut creds = creds.take().unwrap_or_else(|| {
            let mut key = [0u8; 32];
            getrandom::fill(&mut key).expect("OS random number generator");
            Credentials { token: None, db_key: B64.encode(key) }
        });
        if let Some(token) = token {
            creds.token = Some(token.to_owned());
        }
        save_credentials(account, &creds)?;

        let key = B64.decode(&creds.db_key).map_err(|_| StoreError::Corrupt)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(|_| StoreError::Corrupt)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let db = Connection::open(&path)?;
        Self::with_connection(db, cipher, account.clone())
    }

    fn with_connection(db: Connection, cipher: XChaCha20Poly1305, account: AccountId) -> Result<Self, StoreError> {
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS kv (
                 key   TEXT PRIMARY KEY,
                 value BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS messages (
                 id         TEXT PRIMARY KEY,
                 channel_id TEXT NOT NULL,
                 author_id  TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 body       BLOB
             );
             CREATE INDEX IF NOT EXISTS messages_channel ON messages (channel_id, id);",
        )?;
        Ok(Self { db, cipher, account })
    }

    pub fn account(&self) -> &AccountId {
        &self.account
    }

    /// Forgets the session token but keeps the database and its key, so the
    /// history is still readable after logging back in.
    pub fn forget_token(&self) -> Result<(), StoreError> {
        if let Some(mut creds) = load_credentials(&self.account)? {
            creds.token = None;
            save_credentials(&self.account, &creds)?;
        }
        Ok(())
    }

    fn seal(&self, aad: &str, plaintext: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).expect("OS random number generator");
        let ciphertext = self
            .cipher
            .encrypt(&XNonce::from(nonce), Payload { msg: plaintext, aad: aad.as_bytes() })
            .expect("encryption with a valid key cannot fail");
        [nonce.as_slice(), &ciphertext].concat()
    }

    fn open_sealed(&self, aad: &str, sealed: &[u8]) -> Result<Vec<u8>, StoreError> {
        if sealed.len() < NONCE_LEN {
            return Err(StoreError::Corrupt);
        }
        let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
        let nonce = XNonce::try_from(nonce).map_err(|_| StoreError::Corrupt)?;
        self.cipher.decrypt(&nonce, Payload { msg: ciphertext, aad: aad.as_bytes() }).map_err(|_| StoreError::Corrupt)
    }

    fn put(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        let sealed = self.seal(&format!("kv:{key}"), value);
        self.db.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, sealed],
        )?;
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let sealed: Option<Vec<u8>> =
            self.db.query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0)).optional()?;
        sealed.map(|s| self.open_sealed(&format!("kv:{key}"), &s)).transpose()
    }

    pub fn device(&self) -> Result<Option<(DeviceId, Vec<u8>)>, StoreError> {
        let Some(id) = self.get("device_id")? else { return Ok(None) };
        let id = std::str::from_utf8(&id).ok().and_then(|s| s.parse().ok()).ok_or(StoreError::Corrupt)?;
        Ok(self.get("mls_state")?.map(|state| (DeviceId(id), state)))
    }

    pub fn save_device(&self, device: DeviceId, mls_state: &[u8]) -> Result<(), StoreError> {
        let tx = self.db.unchecked_transaction()?;
        self.put("device_id", device.to_string().as_bytes())?;
        self.put("mls_state", mls_state)?;
        tx.commit()?;
        Ok(())
    }

    pub fn save_mls_state(&self, mls_state: &[u8]) -> Result<(), StoreError> {
        self.put("mls_state", mls_state)
    }

    /// Drops the device (e.g. the server no longer knows it). History stays.
    pub fn clear_device(&self) -> Result<(), StoreError> {
        self.db.execute("DELETE FROM kv WHERE key IN ('device_id', 'mls_state')", [])?;
        Ok(())
    }

    /// Stores messages; ones already stored keep their original content.
    pub fn insert_messages(&self, channel: ChannelId, lines: &[ChatLine]) -> Result<(), StoreError> {
        let tx = self.db.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO messages (id, channel_id, author_id, created_at, body) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (id) DO NOTHING",
            )?;
            for line in lines {
                let Some(id) = line.id else { continue };
                let body = line.body.as_ref().map(|b| self.seal(&format!("msg:{id}"), b.as_bytes()));
                stmt.execute(params![
                    id.to_string(),
                    channel.to_string(),
                    line.author.to_string(),
                    line.created_at,
                    body
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The newest `limit` messages of a channel, oldest first.
    pub fn messages(&self, channel: ChannelId, limit: usize) -> Result<Vec<ChatLine>, StoreError> {
        let mut stmt = self.db.prepare_cached(
            "SELECT id, author_id, created_at, body FROM messages WHERE channel_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![channel.to_string(), limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<Vec<u8>>>(3)?))
        })?;
        let mut lines = Vec::new();
        for row in rows {
            let (id, author, created_at, body) = row?;
            let body = match body {
                Some(sealed) => Some(
                    String::from_utf8(self.open_sealed(&format!("msg:{id}"), &sealed)?)
                        .map_err(|_| StoreError::Corrupt)?,
                ),
                None => None,
            };
            lines.push(ChatLine {
                id: Some(MessageId(id.parse().map_err(|_| StoreError::Corrupt)?)),
                author: UserId(author.parse().map_err(|_| StoreError::Corrupt)?),
                created_at,
                body,
            });
        }
        lines.reverse();
        Ok(lines)
    }

    /// ID of the newest stored message in a channel (to catch up from).
    pub fn last_message_id(&self, channel: ChannelId) -> Result<Option<MessageId>, StoreError> {
        let id: Option<String> =
            self.db
                .query_row("SELECT max(id) FROM messages WHERE channel_id = ?1", [channel.to_string()], |r| r.get(0))?;
        id.map(|id| id.parse().map(MessageId).map_err(|_| StoreError::Corrupt)).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let cipher = XChaCha20Poly1305::new_from_slice(&[7u8; 32]).unwrap();
        let account = AccountId { instance: "https://chat.example.com".into(), user_id: UserId::new() };
        Store::with_connection(Connection::open_in_memory().unwrap(), cipher, account).unwrap()
    }

    fn line(body: Option<&str>) -> ChatLine {
        ChatLine {
            id: Some(MessageId::new()),
            author: UserId::new(),
            created_at: 1_700_000_000_000,
            body: body.map(Into::into),
        }
    }

    #[test]
    fn device_state_round_trips_and_is_encrypted() {
        let s = store();
        assert!(s.device().unwrap().is_none());
        let device = DeviceId::new();
        s.save_device(device, b"secret mls state").unwrap();
        assert_eq!(s.device().unwrap(), Some((device, b"secret mls state".to_vec())));

        let raw: Vec<u8> = s.db.query_row("SELECT value FROM kv WHERE key = 'mls_state'", [], |r| r.get(0)).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"secret"));

        s.save_mls_state(b"newer").unwrap();
        assert_eq!(s.device().unwrap().unwrap().1, b"newer");
        s.clear_device().unwrap();
        assert!(s.device().unwrap().is_none());
    }

    #[test]
    fn messages_are_stored_once_in_order() {
        let s = store();
        let channel = ChannelId::new();
        let (a, b, locked) = (line(Some("first")), line(Some("second")), line(None));
        s.insert_messages(channel, &[a.clone(), b.clone(), locked.clone()]).unwrap();
        // Re-inserting (e.g. a refetch that can't decrypt) keeps the stored text.
        s.insert_messages(channel, &[ChatLine { body: None, ..a.clone() }]).unwrap();

        assert_eq!(s.messages(channel, 10).unwrap(), vec![a.clone(), b.clone(), locked.clone()]);
        assert_eq!(s.messages(channel, 2).unwrap(), vec![b, locked.clone()]);
        assert_eq!(s.last_message_id(channel).unwrap(), locked.id);
        assert!(s.messages(ChannelId::new(), 10).unwrap().is_empty());
    }

    #[test]
    fn swapped_ciphertexts_are_rejected() {
        let s = store();
        let channel = ChannelId::new();
        let (a, b) = (line(Some("from a")), line(Some("from b")));
        s.insert_messages(channel, &[a.clone(), b.clone()]).unwrap();
        s.db.execute(
            "UPDATE messages SET body = (SELECT body FROM messages WHERE id = ?1) WHERE id = ?2",
            params![a.id.unwrap().to_string(), b.id.unwrap().to_string()],
        )
        .unwrap();
        assert!(matches!(s.messages(channel, 10), Err(StoreError::Corrupt)));
    }

    #[test]
    fn account_file_names_are_safe() {
        let account = AccountId { instance: "https://chat.example.com:8443/x".into(), user_id: UserId::new() };
        let name = account.file_name();
        assert!(name.starts_with("chat.example.com_8443_x-"));
        assert!(name.ends_with(".db"));
    }
}
