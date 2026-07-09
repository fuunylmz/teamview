pub mod h264;

use crate::capture::CaptureFrame;
use teamview_protocol::{codec::CodecId, frame::EncodedFrame as ProtocolEncodedFrame};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub codec: CodecId,
    pub frame_id: u32,
    pub is_keyframe: bool,
    pub capture_time_micros: u64,
    pub bytes: Vec<u8>,
}

pub trait VideoEncoder {
    fn encode(
        &mut self,
        frame: CaptureFrame,
        stream_id: u32,
    ) -> anyhow::Result<Option<ProtocolEncodedFrame>>;
    fn request_keyframe(&mut self);
    fn update_bitrate(&mut self, bitrate_bps: u32);
}
