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

use sivryn_client_core::ClientError;
use sivryn_client_core::protocol::api::{MessageQuery, MlsCommitRequest, MlsWelcome, SendMessageRequest};
use sivryn_client_core::protocol::model::{Message, MessageContent, MessageKind, Reaction};
use sivryn_client_core::protocol::{ChannelId, MessageId, UserId};
use sivryn_crypto::{Incoming, PendingCommit};

use crate::messages::Record;
use crate::session::Session;

/// Messages shown when a channel opens.
pub const HISTORY_PAGE: usize = 150;
/// Server page size when catching up.
const CATCH_UP_PAGE: u32 = 100;

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
    submit(s, channel, pending, claimed.iter().map(|c| c.device_id).collect()).await
}

/// Removes all devices of `users` from the channel's group (kicked, banned
/// or left), so they can't read anything sent from now on.
pub async fn remove_users(s: &Session, channel: ChannelId, users: &[UserId]) -> Result<(), String> {
    if !s.mls.borrow().has_group(channel) {
        return Ok(());
    }
    let identities: Vec<String> = users.iter().map(|u| u.to_string()).collect();
    let pending = s.mls.borrow_mut().remove_identities(channel, &identities).map_err(|e| e.to_string())?;
    match pending {
        Some(pending) => submit(s, channel, pending, Vec::new()).await,
        None => Ok(()),
    }
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
    let encrypted = s.mls.borrow_mut().encrypt(channel, &content).map_err(|e| e.to_string());
    s.persist_mls();
    let (epoch, ciphertext) = encrypted?;
    let msg = s
        .api
        .send_message(
            channel,
            &SendMessageRequest { device_id: s.device_id, epoch, ciphertext, kind, target_id: target },
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

/// Decrypts one record without storing it. Own messages can't be decrypted
/// by MLS design (the store already has them).
fn decrypt_one(s: &Session, msg: &Message) -> Option<Record> {
    if msg.sender_device_id == s.device_id {
        return None;
    }
    let content = match s.mls.borrow_mut().process(msg.channel_id, &msg.ciphertext) {
        Ok(Incoming::Message(content)) => Some(content),
        Ok(_) => return None,
        Err(e) => {
            tracing::debug!(error = %e, id = %msg.id, "cannot decrypt message");
            None
        }
    };
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
    let records: Vec<Record> = messages.iter().filter_map(|m| decrypt_one(s, m)).collect();
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
    Ok(new)
}
