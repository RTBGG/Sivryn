//! Codec selection for screen share: AV1 when the GPU can encode it, else H.264.

use livekit::options::VideoCodec as LkCodec;
use livekit::webrtc::rtp_sender::VideoEncoderBackend;
use sivryn_protocol::media::VideoCodec;

/// Video codecs the local GPU can encode in hardware.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HardwareEncoders {
    pub av1: bool,
    pub h264: bool,
    pub hevc: bool,
}

impl HardwareEncoders {
    pub fn supports(&self, codec: VideoCodec) -> bool {
        match codec {
            VideoCodec::Av1 => self.av1,
            VideoCodec::H264 => self.h264,
            VideoCodec::Vp8 | VideoCodec::Vp9 => false,
        }
    }

    /// Asks Media Foundation which hardware encoders are installed.
    #[cfg(windows)]
    pub fn detect() -> Self {
        windows_mf::detect().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "hardware encoder detection failed; assuming none");
            Self::default()
        })
    }

    // TODO: VideoToolbox (macOS) and VAAPI/NVENC (Linux) detection.
    #[cfg(not(windows))]
    pub fn detect() -> Self {
        Self::default()
    }
}

/// What the screen share will be encoded with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodecChoice {
    pub codec: VideoCodec,
    pub hardware: bool,
}

/// Picks the first codec in the server's preference list that this machine
/// encodes in hardware and every viewer can decode. Falls back to software
/// H.264, which every WebRTC client decodes.
///
/// `viewers_decode` is the set of codecs all current subscribers support.
/// TODO: re-negotiate when a viewer without AV1 support joins mid-share.
pub fn choose(preference: &[VideoCodec], hw: HardwareEncoders, viewers_decode: &[VideoCodec]) -> CodecChoice {
    preference
        .iter()
        .copied()
        .find(|c| hw.supports(*c) && viewers_decode.contains(c))
        .map(|codec| CodecChoice { codec, hardware: true })
        .unwrap_or(CodecChoice { codec: VideoCodec::H264, hardware: hw.h264 })
}

impl CodecChoice {
    pub fn livekit_codec(&self) -> LkCodec {
        match self.codec {
            VideoCodec::Av1 => LkCodec::AV1,
            VideoCodec::H264 => LkCodec::H264,
            VideoCodec::Vp9 => LkCodec::VP9,
            VideoCodec::Vp8 => LkCodec::VP8,
        }
    }

    pub fn encoder_backend(&self) -> VideoEncoderBackend {
        if self.hardware { VideoEncoderBackend::Hardware } else { VideoEncoderBackend::Software }
    }

    /// AV1/VP9 use SVC with temporal layers only: weak viewers get fewer
    /// frames per second but always full resolution, which keeps text sharp.
    /// H.264 has no SVC in WebRTC and uses simulcast instead.
    pub fn scalability_mode(&self) -> Option<String> {
        match self.codec {
            VideoCodec::Av1 | VideoCodec::Vp9 => Some("L1T3".to_owned()),
            VideoCodec::H264 | VideoCodec::Vp8 => None,
        }
    }

    pub fn simulcast(&self) -> bool {
        self.scalability_mode().is_none()
    }
}

#[cfg(windows)]
mod windows_mf {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::core::GUID;

    use super::HardwareEncoders;

    pub fn detect() -> windows::core::Result<HardwareEncoders> {
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE)? };
        let result = HardwareEncoders {
            av1: has_hw_encoder(MFVideoFormat_AV1),
            h264: has_hw_encoder(MFVideoFormat_H264),
            hevc: has_hw_encoder(MFVideoFormat_HEVC),
        };
        unsafe { MFShutdown()? };
        Ok(result)
    }

    fn has_hw_encoder(subtype: GUID) -> bool {
        let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype };
        let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        let ok = unsafe {
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
                None,
                Some(&output),
                &mut activates,
                &mut count,
            )
        }
        .is_ok();

        if !activates.is_null() {
            // Release the COM objects, then the array Media Foundation allocated.
            unsafe {
                for i in 0..count as usize {
                    std::ptr::drop_in_place(activates.add(i));
                }
                CoTaskMemFree(Some(activates as *const _));
            }
        }
        ok && count > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREF: &[VideoCodec] = &[VideoCodec::Av1, VideoCodec::H264];

    #[test]
    fn prefers_av1_with_hardware_and_capable_viewers() {
        let hw = HardwareEncoders { av1: true, h264: true, hevc: false };
        let choice = choose(PREF, hw, &[VideoCodec::Av1, VideoCodec::H264]);
        assert_eq!(choice, CodecChoice { codec: VideoCodec::Av1, hardware: true });
        assert_eq!(choice.scalability_mode().as_deref(), Some("L1T3"));
    }

    #[test]
    fn falls_back_to_h264_when_a_viewer_lacks_av1() {
        let hw = HardwareEncoders { av1: true, h264: true, hevc: false };
        let choice = choose(PREF, hw, &[VideoCodec::H264]);
        assert_eq!(choice.codec, VideoCodec::H264);
        assert!(choice.simulcast());
    }

    #[test]
    fn software_h264_without_any_hardware() {
        let choice = choose(PREF, HardwareEncoders::default(), &[VideoCodec::Av1, VideoCodec::H264]);
        assert_eq!(choice, CodecChoice { codec: VideoCodec::H264, hardware: false });
    }
}
