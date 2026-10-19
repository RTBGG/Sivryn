//! Remote video for the UI: decoded frames converted to RGBA.

use std::sync::Arc;

use futures::StreamExt;
use livekit::prelude::{RemoteVideoTrack, TrackSource};
use livekit::webrtc::video_frame::VideoFormatType;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use tokio::sync::watch;

/// One decoded frame, tightly packed RGBA8 (ready for a GPU texture upload).
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// A remote camera or screen share. The UI reads the latest frame from
/// `frames`; older frames are dropped, so a slow renderer never builds up lag.
#[derive(Clone)]
pub struct VideoFeed {
    pub participant: String,
    pub source: TrackSource,
    pub track_sid: String,
    pub frames: watch::Receiver<Option<Arc<RgbaFrame>>>,
}

impl PartialEq for VideoFeed {
    fn eq(&self, other: &Self) -> bool {
        self.track_sid == other.track_sid
    }
}

pub(crate) fn spawn_feed(participant: String, source: TrackSource, track: RemoteVideoTrack) -> VideoFeed {
    let (tx, frames) = watch::channel(None);
    let track_sid = track.sid().to_string();
    let mut stream = NativeVideoStream::new(track.rtc_track());
    let track_sid_log = track_sid.clone();

    tokio::spawn(async move {
        let mut logged_first = false;
        while let Some(frame) = stream.next().await {
            let (width, height) = (frame.buffer.width(), frame.buffer.height());
            if !logged_first {
                tracing::info!(%track_sid_log, width, height, "first remote video frame decoded");
                logged_first = true;
            }
            let mut data = vec![0u8; (width * height * 4) as usize];
            // libyuv's "ABGR" is R, G, B, A in memory, i.e. RGBA8.
            frame.buffer.to_argb(VideoFormatType::ABGR, &mut data, width * 4, width as i32, height as i32);
            if tx.send(Some(Arc::new(RgbaFrame { width, height, data }))).is_err() {
                break;
            }
        }
    });

    VideoFeed { participant, source, track_sid, frames }
}
