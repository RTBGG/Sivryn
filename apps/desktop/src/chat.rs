//! End-to-end encrypted chat on top of the MLS delivery service.
//!
//! Group lifecycle (MVP): the first device that opens a channel creates its
//! group and adds every other member's devices; devices added later receive a
//! welcome. When a new member joins a guild, any device already in a channel's
//! group adds them. The server orders competing commits per epoch.

use sivryn_client_core::ClientError;
use sivryn_client_core::protocol::api::{MlsCommitRequest, MlsWelcome, SendMessageRequest};
use sivryn_client_core::protocol::model::{Message, MessageContent};
use sivryn_client_core::protocol::{ChannelId, GuildId, MessageId, UserId};
use sivryn_crypto::Incoming;

use crate::session::Session;

#[derive(Clone, Debug, PartialEq)]
pub struct ChatLine {
    pub id: Option<MessageId>,
    pub author: UserId,
    /// `None` when this device cannot decrypt the message (sent before it
    /// joined the group, or from an epoch whose keys are gone).
    pub body: Option<String>,
}

fn is_conflict(e: &ClientError) -> bool {
    matches!(e, ClientError::Api { status, .. } if status.as_u16() == 409)
}

/// Joins every channel group this device has been welcomed to.
pub async fn accept_welcomes(s: &Session) -> Result<(), String> {
    for pending in s.api.take_welcomes(s.device_id).await.map_err(|e| e.to_string())? {
        let w = pending.welcome;
        if let Err(e) = s.mls.borrow_mut().join(pending.channel_id, &w.welcome, &w.ratchet_tree) {
            tracing::warn!(error = %e, channel = %pending.channel_id, "could not join from welcome");
        }
    }
    Ok(())
}

/// Makes sure this device is in the channel's MLS group.
/// Returns `false` while waiting for another member to add this device.
pub async fn ensure_group(s: &Session, guild: GuildId, channel: ChannelId) -> Result<bool, String> {
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

    s.mls.borrow_mut().create_group(channel).map_err(|e| e.to_string())?;
    let members = s.api.members(guild).await.map_err(|e| e.to_string())?;
    for member in members.into_iter().filter(|m| m.id != s.user.id) {
        add_user(s, channel, member.id).await?;
    }
    Ok(s.mls.borrow().has_group(channel))
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

    let request = MlsCommitRequest {
        device_id: s.device_id,
        epoch: pending.epoch,
        commit: pending.commit,
        welcome: pending.welcome.map(|welcome| MlsWelcome {
            recipients: claimed.iter().map(|c| c.device_id).collect(),
            welcome,
            ratchet_tree: pending.ratchet_tree,
        }),
    };
    match s.api.mls_commit(channel, &request).await {
        Ok(()) => {
            s.mls.borrow_mut().confirm_commit(channel).map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(e) if is_conflict(&e) => {
            // Another device committed first. If we only just created the group
            // we lost the race to create it and wait for their welcome instead.
            let mut mls = s.mls.borrow_mut();
            if pending.epoch == 0 {
                mls.remove_group(channel);
            } else {
                mls.discard_commit(channel).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

pub async fn send(s: &Session, channel: ChannelId, body: String) -> Result<ChatLine, String> {
    let content = MessageContent { body: body.clone(), reply_to: None };
    let (epoch, ciphertext) = s.mls.borrow_mut().encrypt(channel, &content).map_err(|e| e.to_string())?;
    let msg = s
        .api
        .send_message(channel, &SendMessageRequest { device_id: s.device_id, epoch, ciphertext })
        .await
        .map_err(|e| e.to_string())?;
    Ok(ChatLine { id: Some(msg.id), author: s.user.id, body: Some(body) })
}

/// Decrypts a message from the server. Own messages can't be decrypted by
/// MLS design; the UI already shows them from the local echo.
pub fn decrypt(s: &Session, msg: &Message) -> Option<ChatLine> {
    if msg.sender_device_id == s.device_id {
        return None;
    }
    let body = match s.mls.borrow_mut().process(msg.channel_id, &msg.ciphertext) {
        Ok(Incoming::Message(content)) => Some(content.body),
        Ok(_) => return None,
        Err(e) => {
            tracing::debug!(error = %e, id = %msg.id, "cannot decrypt message");
            None
        }
    };
    Some(ChatLine { id: Some(msg.id), author: msg.author_id, body })
}

/// Applies a commit relayed by the gateway.
pub fn apply_commit(s: &Session, channel: ChannelId, commit: &str) -> Option<u64> {
    match s.mls.borrow_mut().process(channel, commit) {
        Ok(Incoming::EpochChanged { epoch }) => Some(epoch),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(error = %e, %channel, "failed to apply commit");
            None
        }
    }
}
