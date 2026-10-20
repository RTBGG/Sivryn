//! A voice channel session: LiveKit room, microphone, camera and remote media.

use std::sync::Arc;

use livekit::e2ee::key_provider::{KeyProvider, KeyProviderOptions};
use livekit::e2ee::{E2eeOptions, EncryptionType};
use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::*;
use livekit::webrtc::audio_source::AudioSourceOptions;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use sivryn_protocol::api::VoiceJoinResponse;
use tokio::sync::mpsc;

use crate::MediaError;
use crate::audio::{AudioEngine, Sound, dsp};
use crate::video::{VideoFeed, spawn_feed};

/// Events the UI cares about during a call.
pub enum CallEvent {
    ParticipantJoined {
        identity: String,
        name: String,
    },
    ParticipantLeft {
        identity: String,
    },
    VideoAdded(VideoFeed),
    VideoRemoved {
        track_sid: String,
    },
    SpeakingChanged {
        speakers: Vec<String>,
    },
    /// A participant's microphone was muted or unmuted.
    MuteChanged {
        identity: String,
        muted: bool,
    },
    /// A participant started or stopped sharing their screen.
    ShareChanged {
        identity: String,
        sharing: bool,
    },
    Disconnected,
}

/// Remote participants' screen share audio gets its own volume key.
pub fn screen_volume_key(identity: &str) -> String {
    format!("{identity}:screen")
}

pub struct VoiceCall {
    pub room: Arc<Room>,
    mic: Option<LocalAudioTrack>,
    engine: AudioEngine,
    key_provider: Option<KeyProvider>,
    /// Opus bitrate for the microphone, set by the guild.
    audio_bitrate: u32,
}

impl VoiceCall {
    /// Connects to the channel's room. `media_key` comes from the channel's
    /// MLS group (`MlsDevice::media_key`) and enables frame encryption.
    pub async fn join(
        join: &VoiceJoinResponse,
        media_key: Option<Vec<u8>>,
        engine: AudioEngine,
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
        // Adaptive stream picks layers from the size of the rendering element,
        // which this client doesn't report, so it would stick to the lowest
        // quality. The SFU's bandwidth estimation still adapts per viewer.
        options.adaptive_stream = false;
        options.dynacast = true;
        options.encryption =
            key_provider.clone().map(|key_provider| E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider });

        let (room, mut room_events) = Room::connect(&join.livekit_url, &join.token, options).await?;
        let room = Arc::new(room);
        if key_provider.is_some() {
            room.e2ee_manager().set_enabled(true);
        }
        engine.set_in_call(true);
        engine.play(Sound::Join);

        let (tx, rx) = mpsc::unbounded_channel();
        // People already in the room.
        for p in room.remote_participants().values() {
            let _ = tx.send(CallEvent::ParticipantJoined { identity: p.identity().to_string(), name: p.name() });
            for publication in p.track_publications().values() {
                match publication.source() {
                    TrackSource::Microphone => {
                        let _ = tx.send(CallEvent::MuteChanged {
                            identity: p.identity().to_string(),
                            muted: publication.is_muted(),
                        });
                    }
                    TrackSource::Screenshare => {
                        let _ = tx.send(CallEvent::ShareChanged { identity: p.identity().to_string(), sharing: true });
                    }
                    _ => {}
                }
            }
        }

        let events_engine = engine.clone();
        tokio::spawn(async move {
            let engine = events_engine;
            while let Some(event) = room_events.recv().await {
                let out = match event {
                    RoomEvent::ParticipantConnected(p) => {
                        engine.play(Sound::Join);
                        CallEvent::ParticipantJoined { identity: p.identity().to_string(), name: p.name() }
                    }
                    RoomEvent::ParticipantDisconnected(p) => {
                        engine.play(Sound::Leave);
                        CallEvent::ParticipantLeft { identity: p.identity().to_string() }
                    }
                    RoomEvent::TrackSubscribed { track: RemoteTrack::Video(video), publication, participant } => {
                        CallEvent::VideoAdded(spawn_feed(
                            participant.identity().to_string(),
                            publication.source(),
                            video,
                        ))
                    }
                    RoomEvent::TrackSubscribed { track: RemoteTrack::Audio(audio), publication, participant } => {
                        let identity = participant.identity().to_string();
                        let key = match publication.source() {
                            TrackSource::ScreenshareAudio => screen_volume_key(&identity),
                            _ => identity,
                        };
                        let stream = NativeAudioStream::new(audio.rtc_track(), dsp::RATE as i32, 2);
                        engine.add_remote(audio.sid().to_string(), key, stream);
                        continue;
                    }
                    RoomEvent::TrackUnsubscribed { track: RemoteTrack::Video(video), .. } => {
                        CallEvent::VideoRemoved { track_sid: video.sid().to_string() }
                    }
                    RoomEvent::TrackUnsubscribed { track: RemoteTrack::Audio(audio), .. } => {
                        engine.remove_remote(&audio.sid().to_string());
                        continue;
                    }
                    RoomEvent::TrackPublished { publication, participant } => match publication.source() {
                        TrackSource::Screenshare => {
                            CallEvent::ShareChanged { identity: participant.identity().to_string(), sharing: true }
                        }
                        TrackSource::Microphone => CallEvent::MuteChanged {
                            identity: participant.identity().to_string(),
                            muted: publication.is_muted(),
                        },
                        _ => continue,
                    },
                    RoomEvent::TrackUnpublished { publication, participant } => match publication.source() {
                        TrackSource::Screenshare => {
                            CallEvent::ShareChanged { identity: participant.identity().to_string(), sharing: false }
                        }
                        TrackSource::Microphone => {
                            CallEvent::MuteChanged { identity: participant.identity().to_string(), muted: true }
                        }
                        _ => continue,
                    },
                    RoomEvent::TrackMuted { participant, publication }
                        if publication.source() == TrackSource::Microphone =>
                    {
                        CallEvent::MuteChanged { identity: participant.identity().to_string(), muted: true }
                    }
                    RoomEvent::TrackUnmuted { participant, publication }
                        if publication.source() == TrackSource::Microphone =>
                    {
                        CallEvent::MuteChanged { identity: participant.identity().to_string(), muted: false }
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

        Ok((Self { room, mic: None, engine, key_provider, audio_bitrate: join.audio_bitrate }, rx))
    }

    /// Publishes the microphone through Sivryn's audio engine (noise filter,
    /// echo cancellation, voice activity or push-to-talk).
    pub async fn enable_microphone(&mut self) -> Result<(), MediaError> {
        if self.mic.is_some() {
            return Ok(());
        }
        // The engine already processes the audio; WebRTC must not do it twice.
        // No queue: frames go straight to the encoder for the lowest latency.
        let source = NativeAudioSource::new(AudioSourceOptions::default(), dsp::RATE, 1, 0);
        let track = LocalAudioTrack::create_audio_track("microphone", RtcAudioSource::Native(source.clone()));
        let options = TrackPublishOptions {
            source: TrackSource::Microphone,
            // The guild's bitrate (128 kbit/s by default): well above the
            // 32 kbit/s typical for voice chat, still tiny next to screen sharing.
            audio_encoding: Some(AudioEncoding { max_bitrate: u64::from(self.audio_bitrate) }),
            dtx: true,
            red: true,
            ..Default::default()
        };
        self.room.local_participant().publish_track(LocalTrack::Audio(track.clone()), options).await?;
        self.engine.attach_microphone(Some(source));
        self.mic = Some(track);
        Ok(())
    }

    /// Mutes the microphone; others see the muted state.
    pub fn set_muted(&self, muted: bool) {
        self.engine.set_muted(muted);
        if let Some(mic) = &self.mic {
            if muted { mic.mute() } else { mic.unmute() }
        }
    }

    pub fn identity(&self) -> String {
        self.room.local_participant().identity().to_string()
    }

    /// Rotates the frame encryption key after an MLS epoch change.
    pub fn update_media_key(&self, key: Vec<u8>, epoch: u64) {
        if let Some(provider) = &self.key_provider {
            // Key indices wrap; LiveKit keeps a small window of recent keys so
            // in-flight frames from the previous epoch still decrypt.
            provider.set_shared_key(key, (epoch % 16) as i32);
        }
    }

    /// Changes the microphone bitrate; a published microphone is published
    /// again so the new bitrate applies right away.
    pub async fn set_audio_bitrate(&mut self, bitrate: u32) -> Result<(), MediaError> {
        if bitrate == self.audio_bitrate {
            return Ok(());
        }
        self.audio_bitrate = bitrate;
        let Some(track) = self.mic.take() else { return Ok(()) };
        let muted = track.is_muted();
        self.engine.attach_microphone(None);
        self.room.local_participant().unpublish_track(&track.sid()).await?;
        self.enable_microphone().await?;
        self.set_muted(muted);
        Ok(())
    }

    pub async fn leave(self) {
        self.engine.attach_microphone(None);
        self.engine.clear_remotes();
        self.engine.play(Sound::Leave);
        self.engine.set_in_call(false);
        if let Err(e) = self.room.close().await {
            tracing::warn!(error = %e, "error while leaving call");
        }
    }
}
