//! A voice channel session: LiveKit room, microphone and remote video feeds.

use std::sync::Arc;

use livekit::e2ee::key_provider::{KeyProvider, KeyProviderOptions};
use livekit::e2ee::{E2eeOptions, EncryptionType};
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use sivryn_protocol::api::VoiceJoinResponse;
use tokio::sync::mpsc;

use crate::MediaError;
use crate::video::{VideoFeed, spawn_feed};

/// Events the UI cares about during a call.
pub enum CallEvent {
    ParticipantJoined { identity: String, name: String },
    ParticipantLeft { identity: String },
    VideoAdded(VideoFeed),
    VideoRemoved { track_sid: String },
    SpeakingChanged { speakers: Vec<String> },
    Disconnected,
}

pub struct VoiceCall {
    pub room: Arc<Room>,
    mic: Option<LocalAudioTrack>,
    // Keeps the platform audio device module alive for the call.
    _audio: Option<PlatformAudio>,
    key_provider: Option<KeyProvider>,
}

impl VoiceCall {
    /// Connects to the channel's room. `media_key` comes from the channel's
    /// MLS group (`MlsDevice::media_key`) and enables frame encryption.
    pub async fn join(
        join: &VoiceJoinResponse,
        media_key: Option<Vec<u8>>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<CallEvent>), MediaError> {
        let key_provider = match (join.e2ee, media_key) {
            (true, Some(key)) => Some(KeyProvider::with_shared_key(KeyProviderOptions::default(), key)),
            (true, None) => {
                return Err(MediaError::Audio("channel requires E2EE but no media key is available".into()));
            }
            (false, _) => None,
        };

        let mut options = RoomOptions::default();
        options.auto_subscribe = true;
        options.adaptive_stream = true;
        options.dynacast = true;
        options.encryption =
            key_provider.clone().map(|key_provider| E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider });

        let (room, mut room_events) = Room::connect(&join.livekit_url, &join.token, options).await?;
        let room = Arc::new(room);
        if key_provider.is_some() {
            room.e2ee_manager().set_enabled(true);
        }

        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(event) = room_events.recv().await {
                let out = match event {
                    RoomEvent::ParticipantConnected(p) => {
                        CallEvent::ParticipantJoined { identity: p.identity().to_string(), name: p.name() }
                    }
                    RoomEvent::ParticipantDisconnected(p) => {
                        CallEvent::ParticipantLeft { identity: p.identity().to_string() }
                    }
                    RoomEvent::TrackSubscribed { track: RemoteTrack::Video(video), publication, participant } => {
                        CallEvent::VideoAdded(spawn_feed(
                            participant.identity().to_string(),
                            publication.source(),
                            video,
                        ))
                    }
                    RoomEvent::TrackUnsubscribed { track: RemoteTrack::Video(video), .. } => {
                        CallEvent::VideoRemoved { track_sid: video.sid().to_string() }
                    }
                    RoomEvent::ActiveSpeakersChanged { speakers } => CallEvent::SpeakingChanged {
                        speakers: speakers.iter().map(|p| p.identity().to_string()).collect(),
                    },
                    RoomEvent::Disconnected { .. } => CallEvent::Disconnected,
                    _ => continue,
                };
                if tx.send(out).is_err() {
                    break;
                }
            }
        });

        Ok((Self { room, mic: None, _audio: None, key_provider }, rx))
    }

    /// Publishes the default microphone with WebRTC's echo cancellation,
    /// noise suppression and gain control.
    pub async fn enable_microphone(&mut self) -> Result<(), MediaError> {
        if self.mic.is_some() {
            return Ok(());
        }
        let audio = PlatformAudio::new().map_err(|e| MediaError::Audio(e.to_string()))?;
        let track = LocalAudioTrack::create_audio_track("microphone", audio.rtc_source());
        let options = TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() };
        self.room.local_participant().publish_track(LocalTrack::Audio(track.clone()), options).await?;
        self.mic = Some(track);
        self._audio = Some(audio);
        Ok(())
    }

    pub fn set_muted(&self, muted: bool) {
        if let Some(mic) = &self.mic {
            if muted { mic.mute() } else { mic.unmute() }
        }
    }

    /// Rotates the frame encryption key after an MLS epoch change.
    pub fn update_media_key(&self, key: Vec<u8>, epoch: u64) {
        if let Some(provider) = &self.key_provider {
            // Key indices wrap; LiveKit keeps a small window of recent keys so
            // in-flight frames from the previous epoch still decrypt.
            provider.set_shared_key(key, (epoch % 16) as i32);
        }
    }

    pub async fn leave(self) {
        if let Err(e) = self.room.close().await {
            tracing::warn!(error = %e, "error while leaving call");
        }
    }
}
