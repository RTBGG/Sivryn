//! End-to-end encrypted chat on top of the MLS delivery service.
//!
//! Every channel with messages (guild text channels, DMs and groups) has one
//! MLS group. The first device that opens a channel creates the group and
//! adds every member's devices; devices added later receive a welcome. When
//! someone joins (or leaves) a guild or group, any device already in the
//! group adds (or removes) them. The server orders competing commits.
//!
//! Every decrypted record goes into the local store before the MLS state
//! that consumed its key is saved, so a crash can only cause a refetch, never
//! a lost message.
//!
//! In guilds that share history, messages also carry a copy sealed with the
//! channel's history key, and whoever adds someone hands them the keys, so
//! they can read what was written before (see `history.rs`).

use sivryn_client_core::ClientError;
use sivryn_client_core::protocol::api::{MessageQuery, MlsCommitRequest, MlsWelcome, SendMessageRequest};
use sivryn_client_core::protocol::model::{HistoryKey, Message, MessageContent, MessageKind, Reaction};
use sivryn_client_core::protocol::{ChannelId, MessageId, UserId};
use sivryn_crypto::{Incoming, PendingCommit};

use crate::history;
use crate::messages::Record;
use crate::session::Session;

/// Messages shown when a channel opens.
pub const HISTORY_PAGE: usize = 150;
/// Server page size when catching up.
const CATCH_UP_PAGE: u32 = 100;
/// How far back new history keys look for messages they unlock (pages).
const BACKFILL_PAGES: usize = 20;

fn is_conflict(e: &ClientError) -> bool {
    matches!(e, ClientError::Api { status, .. } if status.as_u16() == 409)
}

/// Joins every channel group this device has been welcomed to.
pub async fn accept_welcomes(s: &Session) -> Result<Vec<ChannelId>, String> {
    let mut joined = Vec::new();
    for pending in s.api.take_welcomes(s.device_id).await.map_err(|e| e.to_string())? {
        let w = pending.welcome;
        match s.mls.borrow_mut().join(pending.channel_id, &w.welcome, &w.ratchet_tree) {
            Ok(_) => joined.push(pending.channel_id),
            Err(e) => tracing::warn!(error = %e, channel = %pending.channel_id, "could not join from welcome"),
        }
    }
    if !joined.is_empty() {
        s.persist_mls();
    }
    Ok(joined)
}

/// Makes sure this device is in the channel's MLS group.
/// Returns `false` while waiting for another member to add this device.
pub async fn ensure_group(s: &Session, channel: ChannelId) -> Result<bool, String> {
    if s.mls.borrow().has_group(channel) {
        return Ok(true);
    }
    accept_welcomes(s).await?;
    if s.mls.borrow().has_group(channel) {
        return Ok(true);
    }

    // Someone already created the group but hasn't added us yet.
    if !s.api.mls_commits_since(channel, 0).await.map_err(|e| e.to_string())?.is_empty() {
        return Ok(false);
    }

    // Opening a channel can run this twice at once; whichever call gets here
    // second finds the group the first one just created.
    if s.mls.borrow().has_group(channel) {
        return Ok(true);
    }
    s.mls.borrow_mut().create_group(channel).map_err(|e| e.to_string())?;
    s.persist_mls();
    let members = s.api.channel_members(channel).await.map_err(|e| e.to_string())?;
    for member in members.into_iter().filter(|m| m.id != s.user.id) {
        add_user(s, channel, member.id).await?;
    }
    Ok(s.mls.borrow().has_group(channel))
}

/// Sends a commit and applies it if the server accepts it.
async fn submit(
    s: &Session,
    channel: ChannelId,
    pending: PendingCommit,
    recipients: Vec<sivryn_client_core::protocol::DeviceId>,
) -> Result<(), String> {
    let request = MlsCommitRequest {
        device_id: s.device_id,
        epoch: pending.epoch,
        commit: pending.commit,
        welcome: pending.welcome.map(|welcome| MlsWelcome { recipients, welcome, ratchet_tree: pending.ratchet_tree }),
    };
    let result = match s.api.mls_commit(channel, &request).await {
        Ok(()) => s.mls.borrow_mut().confirm_commit(channel).map(|_| ()).map_err(|e| e.to_string()),
        Err(e) if is_conflict(&e) => {
            // Another device committed first. If we only just created the group
            // we lost the race to create it and wait for their welcome instead.
            // (A welcome may already have replaced our group; that one stays.)
            let mut mls = s.mls.borrow_mut();
            if pending.epoch == 0 {
                if mls.is_provisional(channel) {
                    mls.remove_group(channel);
                }
                Ok(())
            } else {
                mls.discard_commit(channel).map_err(|e| e.to_string())
            }
        }
        Err(e) => Err(e.to_string()),
    };
    s.persist_mls();
    result
}

/// Adds all devices of `user` to the channel's group, if this device is in it.
pub async fn add_user(s: &Session, channel: ChannelId, user: UserId) -> Result<(), String> {
    if !s.mls.borrow().has_group(channel) {
        return Ok(());
    }
    let claimed = s.api.claim_key_packages(user).await.map_err(|e| e.to_string())?;
    if claimed.is_empty() {
        return Ok(());
    }
    let key_packages: Vec<String> = claimed.iter().map(|c| c.key_package.clone()).collect();
    let pending = s.mls.borrow_mut().add_members(channel, &key_packages).map_err(|e| e.to_string())?;
    let epoch = pending.epoch;
    submit(s, channel, pending, claimed.iter().map(|c| c.device_id).collect()).await?;
    // The new devices are in the group now: hand them the history keys.
    if s.history_channels.borrow().contains(&channel) && s.mls.borrow().epoch(channel) > Some(epoch) {
        let keys = s.store.history_keys(channel).map_err(|e| e.to_string())?;
        if !keys.is_empty()
            && let Err(e) = share_keys(s, channel, keys).await
        {
            tracing::warn!(error = %e, %channel, "could not hand the history keys to new members");
        }
    }
    Ok(())
}

/// Sends history keys to the group (an MLS message of their own kind).
async fn share_keys(s: &Session, channel: ChannelId, keys: Vec<HistoryKey>) -> Result<(), String> {
    let content = MessageContent { history_keys: keys, ..Default::default() };
    send(s, channel, content, MessageKind::HistoryKeys, None).await.map(|_| ())
}

/// Starts a new history key and hands it to the group (first message in a
/// channel, or after someone was removed, so they can't read what follows).
async fn new_history_key(s: &Session, channel: ChannelId) -> Result<(), String> {
    // Kept only once handed out: a key nobody else has would seal copies
    // nobody can open.
    let key = history::new_key();
    share_keys(s, channel, vec![key.clone()]).await?;
    s.store.add_history_keys(channel, std::slice::from_ref(&key)).map_err(|e| e.to_string())?;
    Ok(())
}

/// Makes the channel's group match who may read the channel: adds people
/// who gained access (or joined) and removes those who lost it.
pub async fn sync_members(s: &Session, channel: ChannelId) -> Result<(), String> {
    if !s.mls.borrow().has_group(channel) {
        return Ok(());
    }
    let allowed: Vec<UserId> =
        s.api.channel_members(channel).await.map_err(|e| e.to_string())?.into_iter().map(|u| u.id).collect();
    let present: Vec<UserId> =
        s.mls.borrow().member_identities(channel).iter().filter_map(|id| id.parse().ok()).collect();
    let gone: Vec<UserId> = present.iter().copied().filter(|u| *u != s.user.id && !allowed.contains(u)).collect();
    if !gone.is_empty() {
        remove_users(s, channel, &gone).await?;
    }
    for user in allowed.into_iter().filter(|u| *u != s.user.id && !present.contains(u)) {
        add_user(s, channel, user).await?;
    }
    Ok(())
}

/// Removes all devices of `users` from the channel's group (kicked, banned
/// or left), so they can't read anything sent from now on.
pub async fn remove_users(s: &Session, channel: ChannelId, users: &[UserId]) -> Result<(), String> {
    if !s.mls.borrow().has_group(channel) {
        return Ok(());
    }
    let identities: Vec<String> = users.iter().map(|u| u.to_string()).collect();
    let pending = s.mls.borrow_mut().remove_identities(channel, &identities).map_err(|e| e.to_string())?;
    let Some(pending) = pending else { return Ok(()) };
    let epoch = pending.epoch;
    submit(s, channel, pending, Vec::new()).await?;
    // They're out: what comes next gets a key they never see.
    if s.history_channels.borrow().contains(&channel)
        && s.mls.borrow().epoch(channel) > Some(epoch)
        && let Err(e) = new_history_key(s, channel).await
    {
        tracing::warn!(error = %e, %channel, "could not start a new history key");
    }
    Ok(())
}

/// Encrypts and sends a message, edit or reaction. Returns it as a record
/// (already stored locally, since own messages can't be decrypted later).
pub async fn send(
    s: &Session,
    channel: ChannelId,
    content: MessageContent,
    kind: MessageKind,
    target: Option<MessageId>,
) -> Result<Record, String> {
    // A copy for members who join later, sealed with the newest history key
    // (the channel's first message starts one). Key messages need no copy.
    let mut history = None;
    if kind != MessageKind::HistoryKeys && s.history_channels.borrow().contains(&channel) {
        // Without a key handed out, the message still goes (without a copy).
        if s.store.history_keys(channel).map_err(|e| e.to_string())?.is_empty()
            && let Err(e) = Box::pin(new_history_key(s, channel)).await
        {
            tracing::warn!(error = %e, %channel, "could not hand out a history key");
        }
        let keys = s.store.history_keys(channel).map_err(|e| e.to_string())?;
        history =
            history::current(&keys).and_then(|key| history::seal(key, channel, s.user.id, kind, target, &content));
    }
    let encrypted = s.mls.borrow_mut().encrypt(channel, &content).map_err(|e| e.to_string());
    s.persist_mls();
    let (epoch, ciphertext) = encrypted?;
    let msg = s
        .api
        .send_message(
            channel,
            &SendMessageRequest { device_id: s.device_id, epoch, ciphertext, history, kind, target_id: target },
        )
        .await
        .map_err(|e| e.to_string())?;
    let record = Record {
        id: msg.id,
        author: s.user.id,
        created_at: msg.created_at.timestamp_millis(),
        kind,
        target,
        content: Some(content),
    };
    if let Err(e) = s.store.insert(channel, std::slice::from_ref(&record)) {
        tracing::warn!(error = %e, "could not store sent message");
    }
    Ok(record)
}

pub async fn react(
    s: &Session,
    channel: ChannelId,
    target: MessageId,
    emoji: &str,
    remove: bool,
) -> Result<Record, String> {
    let content = MessageContent { reaction: Some(Reaction { emoji: emoji.to_owned(), remove }), ..Default::default() };
    send(s, channel, content, MessageKind::Reaction, Some(target)).await
}

pub async fn delete(s: &Session, channel: ChannelId, message: MessageId) -> Result<(), String> {
    s.api.delete_message(channel, message).await.map_err(|e| e.to_string())?;
    if let Err(e) = s.store.delete(message) {
        tracing::warn!(error = %e, "could not delete stored message");
    }
    Ok(())
}

/// Opens a message's history copy with the channel's history keys.
fn open_history(s: &Session, msg: &Message) -> Option<MessageContent> {
    let sealed = msg.history.as_deref()?;
    let keys = s.store.history_keys(msg.channel_id).ok()?;
    history::open(&keys, sealed, msg.channel_id, msg.author_id, msg.kind, msg.target_id)
}

/// Decrypts one record without storing it. Own messages can't be decrypted
/// by MLS design (the store already has them). History keys in it are kept,
/// and messages MLS can't open (sent before this device joined) are tried
/// with them.
fn decrypt_one(s: &Session, msg: &Message) -> Option<Record> {
    if msg.sender_device_id == s.device_id {
        return None;
    }
    let mut content = match s.mls.borrow_mut().process(msg.channel_id, &msg.ciphertext) {
        Ok(Incoming::Message(content)) => Some(content),
        Ok(_) => return None,
        Err(e) => {
            tracing::debug!(error = %e, id = %msg.id, "cannot decrypt message");
            None
        }
    };
    if msg.kind == MessageKind::HistoryKeys {
        // Kept apart from the messages; the record only marks it as seen.
        let keys = content.take().map(|c| c.history_keys).unwrap_or_default();
        match s.store.add_history_keys(msg.channel_id, &keys) {
            Ok(true) => {
                s.history_unlocked.borrow_mut().insert(msg.channel_id);
            }
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %e, "could not store history keys"),
        }
    } else if content.is_none() {
        content = open_history(s, msg);
    }
    Some(Record {
        id: msg.id,
        author: msg.author_id,
        created_at: msg.created_at.timestamp_millis(),
        kind: msg.kind,
        target: msg.target_id,
        content,
    })
}

/// Decrypts and stores records from the server (in order).
pub fn receive(s: &Session, channel: ChannelId, messages: &[Message]) -> Vec<Record> {
    let mut records: Vec<Record> = messages.iter().filter_map(|m| decrypt_one(s, m)).collect();
    // Keys that came later in the same batch open the ones before them.
    for record in records.iter_mut().filter(|r| r.content.is_none() && r.kind != MessageKind::HistoryKeys) {
        if let Some(msg) = messages.iter().find(|m| m.id == record.id) {
            record.content = open_history(s, msg);
        }
    }
    if let Err(e) = s.store.insert(channel, &records) {
        tracing::error!(error = %e, "could not store messages");
    }
    s.persist_mls();
    records
}

/// Applies a commit relayed by the gateway.
pub fn apply_commit(s: &Session, channel: ChannelId, commit: &str) -> Option<u64> {
    let result = s.mls.borrow_mut().process(channel, commit);
    let epoch = match result {
        Ok(Incoming::EpochChanged { epoch }) => Some(epoch),
        Ok(_) => None,
        Err(e) => {
            // Our own commits come back too; they're already applied.
            tracing::debug!(error = %e, %channel, "commit not applied");
            None
        }
    };
    s.persist_mls();
    epoch
}

/// After new history keys arrived: goes back through the channel's messages
/// on the server and stores the ones they unlock (that weren't readable or
/// weren't here at all). Returns those records.
pub async fn backfill(s: &Session, channel: ChannelId) -> Result<Vec<Record>, String> {
    if !s.history_unlocked.borrow_mut().remove(&channel) {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    let mut before = None;
    for _ in 0..BACKFILL_PAGES {
        let page = s
            .api
            .messages(channel, &MessageQuery { before, limit: Some(CATCH_UP_PAGE), ..Default::default() })
            .await
            .map_err(|e| e.to_string())?;
        let Some(oldest) = page.iter().map(|m| m.id).min_by_key(|id| id.0) else { break };
        before = Some(oldest);
        let ids: Vec<MessageId> = page.iter().map(|m| m.id).collect();
        let readable = s.store.readable(&ids).map_err(|e| e.to_string())?;
        let mut unlocked = Vec::new();
        for msg in page.iter().filter(|m| !readable.contains(&m.id) && m.history.is_some()) {
            if let Some(content) = open_history(s, msg) {
                unlocked.push(Record {
                    id: msg.id,
                    author: msg.author_id,
                    created_at: msg.created_at.timestamp_millis(),
                    kind: msg.kind,
                    target: msg.target_id,
                    content: Some(content),
                });
            }
        }
        s.store.insert(channel, &unlocked).map_err(|e| e.to_string())?;
        found.extend(unlocked);
        // Older than history copies (sent before they existed): nothing more to find.
        if page.len() < CATCH_UP_PAGE as usize || page.iter().all(|m| m.history.is_none()) {
            break;
        }
    }
    Ok(found)
}

/// The channel's stored history (newest [`HISTORY_PAGE`] messages).
pub fn history(s: &Session, channel: ChannelId) -> Vec<Record> {
    s.store.records(channel, HISTORY_PAGE).unwrap_or_else(|e| {
        tracing::error!(error = %e, "could not read message history");
        Vec::new()
    })
}

/// Brings a channel up to date after being offline: applies missed commits
/// first (past epochs stay decryptable), then fetches and stores every
/// record newer than the newest stored one. Returns the new records.
pub async fn catch_up(s: &Session, channel: ChannelId) -> Result<Vec<Record>, String> {
    if !s.mls.borrow().has_group(channel) {
        return Ok(Vec::new());
    }
    let epoch = s.mls.borrow().epoch(channel).unwrap_or(0);
    for commit in s.api.mls_commits_since(channel, epoch).await.map_err(|e| e.to_string())? {
        apply_commit(s, channel, &commit.commit);
    }

    let mut after = s.store.last_message_id(channel).map_err(|e| e.to_string())?;
    let mut new = Vec::new();
    if after.is_none() {
        // Nothing stored yet: start from the latest page.
        let mut page = s
            .api
            .messages(channel, &MessageQuery { limit: Some(CATCH_UP_PAGE), ..Default::default() })
            .await
            .map_err(|e| e.to_string())?;
        page.reverse();
        after = page.last().map(|m| m.id);
        new.extend(receive(s, channel, &page));
    }
    while let Some(cursor) = after {
        let page = s
            .api
            .messages(channel, &MessageQuery { after: Some(cursor), limit: Some(CATCH_UP_PAGE), ..Default::default() })
            .await
            .map_err(|e| e.to_string())?;
        let full = page.len() as u32 == CATCH_UP_PAGE;
        after = if full { page.last().map(|m| m.id) } else { None };
        new.extend(receive(s, channel, &page));
    }
    new.extend(backfill(s, channel).await?);
    Ok(new)
}
