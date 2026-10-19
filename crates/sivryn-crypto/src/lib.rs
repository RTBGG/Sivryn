//! End-to-end encryption for Sivryn, built on MLS (RFC 9420) via OpenMLS.
//!
//! Every channel has one MLS group whose members are *devices*, not users.
//! The server acts as the MLS delivery service: it stores key packages,
//! orders commits and relays welcomes, but never learns group secrets.
//!
//! The same group also yields the media key for E2EE voice/video: both sides
//! derive it with the MLS exporter, so it changes with every epoch (i.e. every
//! time someone joins or leaves).

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use openmls::prelude::tls_codec::Deserialize as _;
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::{MemoryStorage, RustCrypto};
use sivryn_protocol::ChannelId;
use sivryn_protocol::model::MessageContent;

const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
const MEDIA_KEY_LABEL: &str = "sivryn media e2ee v1";
const MEDIA_KEY_LEN: usize = 32;
/// Past epochs whose message keys are kept, so messages sent just before a
/// commit (or fetched after catching up on commits) still decrypt.
const MAX_PAST_EPOCHS: usize = 16;
/// Format version of [`MlsDevice::export`].
const EXPORT_VERSION: u8 = 1;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("no MLS group for channel {0}")]
    UnknownChannel(ChannelId),
    #[error("invalid encoding: {0}")]
    Encoding(String),
    #[error("MLS error: {0}")]
    Mls(String),
}

fn mls_err(e: impl std::fmt::Debug) -> CryptoError {
    CryptoError::Mls(format!("{e:?}"))
}

fn decode(b64: &str) -> Result<Vec<u8>, CryptoError> {
    B64.decode(b64).map_err(|e| CryptoError::Encoding(e.to_string()))
}

fn encode(msg: &impl tls_codec::Serialize) -> Result<String, CryptoError> {
    Ok(B64.encode(msg.tls_serialize_detached().map_err(mls_err)?))
}

/// A commit to hand to the server. Call [`MlsDevice::confirm_commit`] once the
/// server accepted it, or [`MlsDevice::discard_commit`] on a conflict.
#[derive(Debug, Clone)]
pub struct PendingCommit {
    /// The epoch the commit was created in (the server orders by this).
    pub epoch: u64,
    pub commit: String,
    pub welcome: Option<String>,
    pub ratchet_tree: String,
}

/// Result of processing a message from the server.
#[derive(Debug)]
pub enum Incoming {
    Message(MessageContent),
    /// A commit was applied; the group (and media key) moved to `epoch`.
    EpochChanged {
        epoch: u64,
    },
    /// Our own message echoed back by the server.
    Own,
    /// Proposals and other handshake messages that need no UI action.
    Ignored,
}

/// OpenMLS provider whose in-memory storage can be exported and restored.
#[derive(Default)]
struct Provider {
    crypto: RustCrypto,
    storage: MemoryStorage,
}

impl OpenMlsProvider for Provider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

fn create_config() -> MlsGroupCreateConfig {
    MlsGroupCreateConfig::builder().max_past_epochs(MAX_PAST_EPOCHS).build()
}

fn join_config() -> MlsGroupJoinConfig {
    MlsGroupJoinConfig::builder().max_past_epochs(MAX_PAST_EPOCHS).build()
}

/// The MLS state of one device. Persist it with [`MlsDevice::export`] after
/// every change and restore it with [`MlsDevice::import`].
pub struct MlsDevice {
    provider: Provider,
    signer: SignatureKeyPair,
    credential: CredentialWithKey,
    groups: HashMap<ChannelId, MlsGroup>,
}

impl MlsDevice {
    /// Creates a new device identity. `identity` should be `"<user_id>:<device_id>"`.
    pub fn new(identity: &str) -> Result<Self, CryptoError> {
        let provider = Provider::default();
        let signer = SignatureKeyPair::new(CIPHERSUITE.signature_algorithm()).map_err(mls_err)?;
        signer.store(provider.storage()).map_err(mls_err)?;
        let credential = CredentialWithKey {
            credential: BasicCredential::new(identity.as_bytes().to_vec()).into(),
            signature_key: signer.public().into(),
        };
        Ok(Self { provider, signer, credential, groups: HashMap::new() })
    }

    /// Serializes the whole device (identity, signing key, groups) for storage.
    /// The output contains private keys: store it encrypted.
    pub fn export(&self) -> Vec<u8> {
        let mut out = vec![EXPORT_VERSION];
        let mut put = |bytes: &[u8]| {
            out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            out.extend_from_slice(bytes);
        };
        put(self.credential.credential.serialized_content());
        put(self.signer.public());
        put(&(self.signer.signature_scheme() as u16).to_be_bytes());
        let channels: Vec<u8> = self.groups.keys().flat_map(|c| *c.0.as_bytes()).collect();
        put(&channels);
        let values = self.provider.storage.values.read().expect("storage lock");
        put(&(values.len() as u64).to_be_bytes());
        for (k, v) in values.iter() {
            put(k);
            put(v);
        }
        drop(values);
        out
    }

    /// Restores a device from [`MlsDevice::export`].
    pub fn import(data: &[u8]) -> Result<Self, CryptoError> {
        let bad = || CryptoError::Encoding("corrupt device state".into());
        let (&version, mut rest) = data.split_first().ok_or_else(bad)?;
        if version != EXPORT_VERSION {
            return Err(CryptoError::Encoding(format!("unsupported device state version {version}")));
        }
        let mut take = || -> Result<&[u8], CryptoError> {
            let len = rest.get(..8).ok_or_else(bad)?;
            let len = u64::from_be_bytes(len.try_into().expect("8 bytes")) as usize;
            let bytes = rest.get(8..8usize.checked_add(len).ok_or_else(bad)?).ok_or_else(bad)?;
            rest = &rest[8 + len..];
            Ok(bytes)
        };
        let identity = take()?.to_vec();
        let public_key = take()?.to_vec();
        let scheme = u16::from_be_bytes(take()?.try_into().map_err(|_| bad())?);
        let scheme = SignatureScheme::try_from(scheme).map_err(mls_err)?;
        let channels: Vec<ChannelId> =
            take()?.as_chunks::<16>().0.iter().map(|c| ChannelId(uuid::Uuid::from_bytes(*c))).collect();
        let count = u64::from_be_bytes(take()?.try_into().map_err(|_| bad())?);
        let mut values = HashMap::new();
        for _ in 0..count {
            let k = take()?.to_vec();
            values.insert(k, take()?.to_vec());
        }

        let provider = Provider { crypto: RustCrypto::default(), storage: MemoryStorage { values: values.into() } };
        let signer = SignatureKeyPair::read(provider.storage(), &public_key, scheme)
            .ok_or_else(|| CryptoError::Encoding("signing key missing from device state".into()))?;
        let credential = CredentialWithKey {
            credential: BasicCredential::new(identity).into(),
            signature_key: signer.public().into(),
        };
        let mut groups = HashMap::new();
        for channel in channels {
            let group_id = GroupId::from_slice(channel.0.as_bytes());
            if let Some(group) = MlsGroup::load(provider.storage(), &group_id).map_err(mls_err)? {
                groups.insert(channel, group);
            }
        }
        Ok(Self { provider, signer, credential, groups })
    }

    /// Channels this device has a group for.
    pub fn channels(&self) -> impl Iterator<Item = ChannelId> + '_ {
        self.groups.keys().copied()
    }

    /// Generates `count` one-time key packages to upload to the server.
    pub fn key_packages(&self, count: usize) -> Result<Vec<String>, CryptoError> {
        (0..count)
            .map(|_| {
                let bundle = KeyPackage::builder()
                    .build(CIPHERSUITE, &self.provider, &self.signer, self.credential.clone())
                    .map_err(mls_err)?;
                encode(bundle.key_package())
            })
            .collect()
    }

    pub fn has_group(&self, channel: ChannelId) -> bool {
        self.groups.contains_key(&channel)
    }

    pub fn epoch(&self, channel: ChannelId) -> Option<u64> {
        self.groups.get(&channel).map(|g| g.epoch().as_u64())
    }

    /// Forgets a channel's group, e.g. after losing the race to create it.
    pub fn remove_group(&mut self, channel: ChannelId) {
        if let Some(mut group) = self.groups.remove(&channel) {
            let _ = group.delete(self.provider.storage());
        }
    }

    /// Starts a new group for a channel with this device as the only member.
    pub fn create_group(&mut self, channel: ChannelId) -> Result<(), CryptoError> {
        let group = MlsGroup::new_with_group_id(
            &self.provider,
            &self.signer,
            &create_config(),
            GroupId::from_slice(channel.0.as_bytes()),
            self.credential.clone(),
        )
        .map_err(mls_err)?;
        self.groups.insert(channel, group);
        Ok(())
    }

    /// Adds devices (from claimed key packages) to a channel's group.
    pub fn add_members(&mut self, channel: ChannelId, key_packages: &[String]) -> Result<PendingCommit, CryptoError> {
        let key_packages = key_packages
            .iter()
            .map(|kp| {
                let bytes = decode(kp)?;
                KeyPackageIn::tls_deserialize(&mut bytes.as_slice())
                    .map_err(mls_err)?
                    .validate(self.provider.crypto(), ProtocolVersion::Mls10)
                    .map_err(mls_err)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let provider = &self.provider;
        let signer = &self.signer;
        let group = self.groups.get_mut(&channel).ok_or(CryptoError::UnknownChannel(channel))?;
        let epoch = group.epoch().as_u64();
        let (commit, welcome, _group_info) = group.add_members(provider, signer, &key_packages).map_err(mls_err)?;

        // The ratchet tree joiners need is the one *after* this commit.
        let ratchet_tree = group
            .pending_commit()
            .map(|staged| staged.export_ratchet_tree(provider.crypto(), group.export_ratchet_tree()))
            .transpose()
            .map_err(mls_err)?
            .flatten()
            .unwrap_or_else(|| group.export_ratchet_tree());

        Ok(PendingCommit {
            epoch,
            commit: encode(&commit)?,
            welcome: Some(encode(&welcome)?),
            ratchet_tree: encode(&ratchet_tree)?,
        })
    }

    /// The server accepted our commit: apply it locally.
    pub fn confirm_commit(&mut self, channel: ChannelId) -> Result<u64, CryptoError> {
        let provider = &self.provider;
        let group = self.groups.get_mut(&channel).ok_or(CryptoError::UnknownChannel(channel))?;
        group.merge_pending_commit(provider).map_err(mls_err)?;
        Ok(group.epoch().as_u64())
    }

    /// Another commit won the epoch: drop ours, process theirs, then retry.
    pub fn discard_commit(&mut self, channel: ChannelId) -> Result<(), CryptoError> {
        let provider = &self.provider;
        let group = self.groups.get_mut(&channel).ok_or(CryptoError::UnknownChannel(channel))?;
        group.clear_pending_commit(provider.storage()).map_err(mls_err)
    }

    /// Joins a channel's group from a welcome relayed by the server.
    pub fn join(&mut self, channel: ChannelId, welcome: &str, ratchet_tree: &str) -> Result<u64, CryptoError> {
        let welcome = match MlsMessageIn::tls_deserialize(&mut decode(welcome)?.as_slice()).map_err(mls_err)?.extract()
        {
            MlsMessageBodyIn::Welcome(w) => w,
            _ => return Err(CryptoError::Encoding("not a welcome message".into())),
        };
        let tree = RatchetTreeIn::tls_deserialize(&mut decode(ratchet_tree)?.as_slice()).map_err(mls_err)?;
        let group = StagedWelcome::new_from_welcome(&self.provider, &join_config(), welcome, Some(tree))
            .map_err(mls_err)?
            .into_group(&self.provider)
            .map_err(mls_err)?;
        let epoch = group.epoch().as_u64();
        self.groups.insert(channel, group);
        Ok(epoch)
    }

    /// Encrypts a chat message. Returns the epoch and base64 ciphertext.
    pub fn encrypt(&mut self, channel: ChannelId, content: &MessageContent) -> Result<(u64, String), CryptoError> {
        let plaintext = serde_json::to_vec(content).map_err(|e| CryptoError::Encoding(e.to_string()))?;
        let provider = &self.provider;
        let signer = &self.signer;
        let group = self.groups.get_mut(&channel).ok_or(CryptoError::UnknownChannel(channel))?;
        let msg = group.create_message(provider, signer, &plaintext).map_err(mls_err)?;
        Ok((group.epoch().as_u64(), encode(&msg)?))
    }

    /// Processes a message or commit for a channel.
    pub fn process(&mut self, channel: ChannelId, data: &str) -> Result<Incoming, CryptoError> {
        let message = MlsMessageIn::tls_deserialize(&mut decode(data)?.as_slice())
            .map_err(mls_err)?
            .try_into_protocol_message()
            .map_err(mls_err)?;
        let provider = &self.provider;
        let group = self.groups.get_mut(&channel).ok_or(CryptoError::UnknownChannel(channel))?;

        let processed = group.process_message(provider, message).map_err(mls_err)?;

        match processed.into_content() {
            // Our own messages come back through the gateway.
            ProcessedMessageContent::OwnPrivateMessage => Ok(Incoming::Own),
            ProcessedMessageContent::ApplicationMessage(app) => {
                let content =
                    serde_json::from_slice(&app.into_bytes()).map_err(|e| CryptoError::Encoding(e.to_string()))?;
                Ok(Incoming::Message(content))
            }
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                group.merge_staged_commit(provider, *staged).map_err(mls_err)?;
                Ok(Incoming::EpochChanged { epoch: group.epoch().as_u64() })
            }
            _ => Ok(Incoming::Ignored),
        }
    }

    /// Key for LiveKit frame encryption in this channel's current epoch.
    pub fn media_key(&self, channel: ChannelId) -> Result<Vec<u8>, CryptoError> {
        let group = self.groups.get(&channel).ok_or(CryptoError::UnknownChannel(channel))?;
        group
            .export_secret(self.provider.crypto(), MEDIA_KEY_LABEL, channel.0.as_bytes(), MEDIA_KEY_LEN)
            .map_err(mls_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(body: &str) -> MessageContent {
        MessageContent { body: body.into(), reply_to: None }
    }

    #[test]
    fn two_devices_exchange_messages_and_share_media_key() {
        let channel = ChannelId::new();
        let mut alice = MlsDevice::new("alice:1").unwrap();
        let mut bob = MlsDevice::new("bob:1").unwrap();

        alice.create_group(channel).unwrap();
        let bob_kp = bob.key_packages(1).unwrap();
        let pending = alice.add_members(channel, &bob_kp).unwrap();
        assert_eq!(pending.epoch, 0);
        assert_eq!(alice.confirm_commit(channel).unwrap(), 1);

        let epoch = bob.join(channel, pending.welcome.as_deref().unwrap(), &pending.ratchet_tree).unwrap();
        assert_eq!(epoch, 1);

        let (epoch, ciphertext) = alice.encrypt(channel, &content("hello bob")).unwrap();
        assert_eq!(epoch, 1);
        match bob.process(channel, &ciphertext).unwrap() {
            Incoming::Message(m) => assert_eq!(m.body, "hello bob"),
            other => panic!("unexpected {other:?}"),
        }

        let (_, reply) = bob.encrypt(channel, &content("hi alice")).unwrap();
        assert!(matches!(alice.process(channel, &reply).unwrap(), Incoming::Message(m) if m.body == "hi alice"));

        assert_eq!(alice.media_key(channel).unwrap(), bob.media_key(channel).unwrap());
    }

    #[test]
    fn device_survives_export_and_import() {
        let channel = ChannelId::new();
        let mut alice = MlsDevice::new("alice:1").unwrap();
        let bob = MlsDevice::new("bob:1").unwrap();
        alice.create_group(channel).unwrap();
        let pending = alice.add_members(channel, &bob.key_packages(1).unwrap()).unwrap();
        alice.confirm_commit(channel).unwrap();

        // Bob's key package was created before the export, so the restored
        // device must still hold its private key to accept the welcome.
        let mut bob = MlsDevice::import(&bob.export()).unwrap();
        bob.join(channel, pending.welcome.as_deref().unwrap(), &pending.ratchet_tree).unwrap();
        let mut bob = MlsDevice::import(&bob.export()).unwrap();
        assert!(bob.has_group(channel));
        assert_eq!(bob.channels().collect::<Vec<_>>(), vec![channel]);

        let (_, ciphertext) = alice.encrypt(channel, &content("after restart")).unwrap();
        assert!(
            matches!(bob.process(channel, &ciphertext).unwrap(), Incoming::Message(m) if m.body == "after restart")
        );
        let (_, reply) = bob.encrypt(channel, &content("still here")).unwrap();
        assert!(matches!(alice.process(channel, &reply).unwrap(), Incoming::Message(m) if m.body == "still here"));
        assert_eq!(alice.media_key(channel).unwrap(), bob.media_key(channel).unwrap());
    }

    #[test]
    fn messages_from_a_past_epoch_still_decrypt() {
        let channel = ChannelId::new();
        let mut alice = MlsDevice::new("alice:1").unwrap();
        let mut bob = MlsDevice::new("bob:1").unwrap();
        let carol = MlsDevice::new("carol:1").unwrap();
        alice.create_group(channel).unwrap();
        let pending = alice.add_members(channel, &bob.key_packages(1).unwrap()).unwrap();
        alice.confirm_commit(channel).unwrap();
        bob.join(channel, pending.welcome.as_deref().unwrap(), &pending.ratchet_tree).unwrap();

        // Alice sends, then adds Carol. Bob applies the commit first (as when
        // catching up after being offline) and reads the older message after.
        let (_, old) = alice.encrypt(channel, &content("before carol")).unwrap();
        let add = alice.add_members(channel, &carol.key_packages(1).unwrap()).unwrap();
        alice.confirm_commit(channel).unwrap();
        assert!(matches!(bob.process(channel, &add.commit).unwrap(), Incoming::EpochChanged { epoch: 2 }));
        assert!(matches!(bob.process(channel, &old).unwrap(), Incoming::Message(m) if m.body == "before carol"));
    }

    #[test]
    fn corrupt_state_is_rejected() {
        let state = MlsDevice::new("alice:1").unwrap().export();
        assert!(MlsDevice::import(&state[..state.len() / 2]).is_err());
        assert!(MlsDevice::import(&[]).is_err());
    }

    #[test]
    fn media_key_rotates_when_members_join() {
        let channel = ChannelId::new();
        let mut alice = MlsDevice::new("alice:1").unwrap();
        let carol = MlsDevice::new("carol:1").unwrap();

        alice.create_group(channel).unwrap();
        let before = alice.media_key(channel).unwrap();
        alice.add_members(channel, &carol.key_packages(1).unwrap()).unwrap();
        alice.confirm_commit(channel).unwrap();
        assert_ne!(before, alice.media_key(channel).unwrap());
    }
}
