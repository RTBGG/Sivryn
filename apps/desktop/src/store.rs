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
use sivryn_client_core::protocol::model::{HistoryKey, MessageContent, MessageKind};
use sivryn_client_core::protocol::{ChannelId, DeviceId, MessageId, UserId};

use crate::messages::Record;

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
        // Version 2: edits and reactions are stored as records pointing at
        // their message; `body` holds the sealed JSON content.
        let has_kind = db.prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name = 'kind'")?.exists([])?;
        if !has_kind {
            db.execute_batch(
                "ALTER TABLE messages ADD COLUMN kind TEXT NOT NULL DEFAULT 'message';
                 ALTER TABLE messages ADD COLUMN target_id TEXT;",
            )?;
        }
        db.execute_batch("CREATE INDEX IF NOT EXISTS messages_target ON messages (target_id);")?;
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

    /// Stores records. Ones already stored keep their content; only a
    /// message stored unreadable gets the content found later (history keys).
    pub fn insert(&self, channel: ChannelId, records: &[Record]) -> Result<(), StoreError> {
        let tx = self.db.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO messages (id, channel_id, author_id, created_at, body, kind, target_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (id) DO UPDATE SET body = excluded.body
                 WHERE messages.body IS NULL AND excluded.body IS NOT NULL",
            )?;
            for r in records {
                let body = r.content.as_ref().map(|c| {
                    let json = serde_json::to_vec(c).expect("content serializes");
                    self.seal(&format!("msg:{}", r.id), &json)
                });
                stmt.execute(params![
                    r.id.to_string(),
                    channel.to_string(),
                    r.author.to_string(),
                    r.created_at,
                    body,
                    r.kind.as_str(),
                    r.target.map(|t| t.to_string()),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets a deleted message with its edits and reactions.
    pub fn delete(&self, message: MessageId) -> Result<(), StoreError> {
        let id = message.to_string();
        self.db.execute("DELETE FROM messages WHERE id = ?1 OR target_id = ?1", [&id])?;
        Ok(())
    }

    fn row_to_record(
        &self,
        row: (String, String, i64, Option<Vec<u8>>, String, Option<String>),
    ) -> Result<Record, StoreError> {
        let (id, author, created_at, body, kind, target) = row;
        let content = match body {
            Some(sealed) => {
                let plain = self.open_sealed(&format!("msg:{id}"), &sealed)?;
                // Version 1 stored the bare text.
                Some(serde_json::from_slice::<MessageContent>(&plain).unwrap_or_else(|_| MessageContent {
                    body: String::from_utf8_lossy(&plain).into_owned(),
                    ..Default::default()
                }))
            }
            None => None,
        };
        Ok(Record {
            id: MessageId(id.parse().map_err(|_| StoreError::Corrupt)?),
            author: UserId(author.parse().map_err(|_| StoreError::Corrupt)?),
            created_at,
            kind: MessageKind::parse(&kind),
            target: target.map(|t| t.parse().map(MessageId).map_err(|_| StoreError::Corrupt)).transpose()?,
            content,
        })
    }

    /// The newest `limit` messages of a channel plus their edits and reactions.
    pub fn records(&self, channel: ChannelId, limit: usize) -> Result<Vec<Record>, StoreError> {
        let mut stmt = self.db.prepare_cached(
            "WITH recent AS (
                 SELECT id FROM messages WHERE channel_id = ?1 AND kind = 'message' ORDER BY id DESC LIMIT ?2
             )
             SELECT id, author_id, created_at, body, kind, target_id FROM messages
             WHERE id IN (SELECT id FROM recent) OR target_id IN (SELECT id FROM recent)",
        )?;
        let rows = stmt.query_map(params![channel.to_string(), limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(self.row_to_record(row?)?);
        }
        Ok(out)
    }

    /// One stored message (e.g. the target of a reply), if present.
    pub fn record(&self, id: MessageId) -> Result<Option<Record>, StoreError> {
        let row = self
            .db
            .query_row(
                "SELECT id, author_id, created_at, body, kind, target_id FROM messages WHERE id = ?1",
                [id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        row.map(|r| self.row_to_record(r)).transpose()
    }

    /// A channel's history keys (see `history.rs`).
    pub fn history_keys(&self, channel: ChannelId) -> Result<Vec<HistoryKey>, StoreError> {
        match self.get(&format!("history:{channel}"))? {
            Some(json) => serde_json::from_slice(&json).map_err(|_| StoreError::Corrupt),
            None => Ok(Vec::new()),
        }
    }

    /// Adds history keys; returns whether any was new.
    pub fn add_history_keys(&self, channel: ChannelId, keys: &[HistoryKey]) -> Result<bool, StoreError> {
        let mut all = self.history_keys(channel)?;
        let before = all.len();
        for key in keys {
            if !all.iter().any(|k| k.id == key.id) {
                all.push(key.clone());
            }
        }
        if all.len() == before {
            return Ok(false);
        }
        self.put(&format!("history:{channel}"), &serde_json::to_vec(&all).expect("keys serialize"))?;
        Ok(true)
    }

    /// Which of these messages are stored readable.
    pub fn readable(&self, ids: &[MessageId]) -> Result<std::collections::HashSet<MessageId>, StoreError> {
        let mut stmt = self.db.prepare_cached("SELECT 1 FROM messages WHERE id = ?1 AND body IS NOT NULL")?;
        let mut out = std::collections::HashSet::new();
        for id in ids {
            if stmt.exists([id.to_string()])? {
                out.insert(*id);
            }
        }
        Ok(out)
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

    fn record(body: Option<&str>) -> Record {
        Record {
            id: MessageId::new(),
            author: UserId::new(),
            created_at: 1_700_000_000_000,
            kind: MessageKind::Message,
            target: None,
            content: body.map(|b| MessageContent { body: b.into(), ..Default::default() }),
        }
    }

    #[test]
    fn history_keys_collect_and_unlock_stored_messages() {
        let s = store();
        let channel = ChannelId::new();
        let (k1, k2) = (crate::history::new_key(), crate::history::new_key());
        assert!(s.history_keys(channel).unwrap().is_empty());
        assert!(s.add_history_keys(channel, std::slice::from_ref(&k1)).unwrap());
        assert!(!s.add_history_keys(channel, std::slice::from_ref(&k1)).unwrap(), "known key");
        assert!(s.add_history_keys(channel, &[k1.clone(), k2.clone()]).unwrap());
        assert_eq!(s.history_keys(channel).unwrap(), vec![k1, k2]);
        assert!(s.history_keys(ChannelId::new()).unwrap().is_empty());

        // A message stored unreadable gets its content once a key opens it.
        let locked = record(None);
        s.insert(channel, std::slice::from_ref(&locked)).unwrap();
        assert!(s.readable(&[locked.id]).unwrap().is_empty());
        let opened = Record { content: Some(MessageContent { body: "later".into(), ..Default::default() }), ..locked };
        s.insert(channel, std::slice::from_ref(&opened)).unwrap();
        assert!(s.readable(&[opened.id]).unwrap().contains(&opened.id));
        assert_eq!(s.record(opened.id).unwrap().unwrap().content.unwrap().body, "later");
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
    fn records_are_stored_once_with_their_actions() {
        let s = store();
        let channel = ChannelId::new();
        let (a, b, locked) = (record(Some("first")), record(Some("second")), record(None));
        let reaction = Record { kind: MessageKind::Reaction, target: Some(a.id), ..record(Some("")) };
        s.insert(channel, &[a.clone(), b.clone(), locked.clone(), reaction.clone()]).unwrap();
        // Re-inserting (e.g. a refetch that can't decrypt) keeps the stored content.
        s.insert(channel, &[Record { content: None, ..a.clone() }]).unwrap();

        let mut got = s.records(channel, 10).unwrap();
        got.sort_by_key(|r| r.id.0);
        let mut want = vec![a.clone(), b.clone(), locked.clone(), reaction.clone()];
        want.sort_by_key(|r| r.id.0);
        assert_eq!(got, want);
        // The limit counts messages; their reactions come along.
        let newest = s.records(channel, 2).unwrap();
        assert_eq!(newest.len(), 2);
        assert!(newest.iter().all(|r| r.id == b.id || r.id == locked.id));
        assert_eq!(s.last_message_id(channel).unwrap(), Some(reaction.id));
        assert_eq!(s.record(b.id).unwrap(), Some(b));

        s.delete(a.id).unwrap();
        assert!(s.records(channel, 10).unwrap().iter().all(|r| r.id != a.id && r.id != reaction.id));
        assert!(s.records(ChannelId::new(), 10).unwrap().is_empty());
    }

    #[test]
    fn version_one_rows_still_load() {
        let s = store();
        let channel = ChannelId::new();
        let id = MessageId::new();
        let sealed = s.seal(&format!("msg:{id}"), b"plain old text");
        s.db.execute(
            "INSERT INTO messages (id, channel_id, author_id, created_at, body) VALUES (?1, ?2, ?3, 1, ?4)",
            params![id.to_string(), channel.to_string(), UserId::new().to_string(), sealed],
        )
        .unwrap();
        let got = s.records(channel, 10).unwrap();
        assert_eq!(got[0].content.as_ref().unwrap().body, "plain old text");
        assert_eq!(got[0].kind, MessageKind::Message);
    }

    #[test]
    fn swapped_ciphertexts_are_rejected() {
        let s = store();
        let channel = ChannelId::new();
        let (a, b) = (record(Some("from a")), record(Some("from b")));
        s.insert(channel, &[a.clone(), b.clone()]).unwrap();
        s.db.execute(
            "UPDATE messages SET body = (SELECT body FROM messages WHERE id = ?1) WHERE id = ?2",
            params![a.id.to_string(), b.id.to_string()],
        )
        .unwrap();
        assert!(matches!(s.records(channel, 10), Err(StoreError::Corrupt)));
    }

    #[test]
    fn account_file_names_are_safe() {
        let account = AccountId { instance: "https://chat.example.com:8443/x".into(), user_id: UserId::new() };
        let name = account.file_name();
        assert!(name.starts_with("chat.example.com_8443_x-"));
        assert!(name.ends_with(".db"));
    }
}
