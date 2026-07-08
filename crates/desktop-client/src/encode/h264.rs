use super::VideoEncoder;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H264EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub frames_per_second: u16,
    pub bitrate_bps: u32,
}

impl Default for H264EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            frames_per_second: 30,
            bitrate_bps: 4_000_000,
        }
    }
}

#[derive(Debug, Default)]
pub struct H264Encoder {
    pub config: H264EncoderConfig,
    pub keyframe_requested: bool,
}

impl VideoEncoder for H264Encoder {
    fn request_keyframe(&mut self) {
        self.keyframe_requested = true;
    }

    fn update_bitrate(&mut self, bitrate_bps: u32) {
        self.config.bitrate_bps = bitrate_bps;
    }
}
