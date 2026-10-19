//! Prints what this machine offers for screen sharing:
//! `cargo run -p sivryn-media --example probe`

use sivryn_media::codec::{self, HardwareEncoders};
use sivryn_protocol::media::ScreenShareLimits;

fn main() {
    let hw = HardwareEncoders::detect();
    println!("Hardware encoders: AV1={} H.264={} HEVC={}", hw.av1, hw.h264, hw.hevc);

    let limits = ScreenShareLimits::default();
    let choice = codec::choose(&limits.codec_preference, hw, &limits.codec_preference);
    println!("Screen share would use: {:?} (hardware: {})", choice.codec, choice.hardware);

    let sources = sivryn_media::list_sources();
    println!("{} capture sources:", sources.len());
    for s in sources.iter().take(15) {
        println!("  [{:?}] {} (id {})", s.kind, s.title, s.id);
    }
}
