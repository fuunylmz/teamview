use bytes::Bytes;
use teamview_protocol::{codec::CodecId, frame::EncodedFrame};

use crate::capture::CaptureFrame;

use super::VideoEncoder;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H264EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub frames_per_second: u16,
    pub bitrate_bps: u32,
    pub synthetic_payload_bytes: usize,
}

impl Default for H264EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            frames_per_second: 30,
            bitrate_bps: 4_000_000,
            synthetic_payload_bytes: 512,
        }
    }
}

#[derive(Debug, Default)]
pub struct H264Encoder {
    pub config: H264EncoderConfig,
    pub keyframe_requested: bool,
}

impl VideoEncoder for H264Encoder {
    fn encode(
        &mut self,
        frame: CaptureFrame,
        stream_id: u32,
    ) -> anyhow::Result<Option<EncodedFrame>> {
        let is_keyframe = self.keyframe_requested || frame.frame_id == 1;
        self.keyframe_requested = false;

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f]);
        if is_keyframe {
            bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65]);
        } else {
            bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x41]);
        }
        bytes.extend_from_slice(&frame.frame_id.to_le_bytes());
        bytes.extend_from_slice(&frame.width.to_le_bytes());
        bytes.extend_from_slice(&frame.height.to_le_bytes());
        while bytes.len() < self.config.synthetic_payload_bytes {
            bytes.push(
                frame
                    .frame_id
                    .wrapping_add(bytes.len() as u64)
                    .to_le_bytes()[0],
            );
        }
        bytes.truncate(self.config.synthetic_payload_bytes);

        Ok(Some(EncodedFrame {
            room_stream_id: stream_id,
            frame_id: frame.frame_id as u32,
            media_timestamp: frame.frame_id.saturating_mul(3_000),
            sender_capture_time_micros: frame.capture_time_micros,
            codec: CodecId::H264,
            is_keyframe,
            bytes: Bytes::from(bytes),
        }))
    }

    fn request_keyframe(&mut self) {
        self.keyframe_requested = true;
    }

    fn update_bitrate(&mut self, bitrate_bps: u32) {
        self.config.bitrate_bps = bitrate_bps;
    }
}

#[cfg(test)]
mod tests {
    use crate::capture::CaptureFrame;

    use super::*;

    #[test]
    fn synthetic_encoder_outputs_protocol_frame() {
        let mut encoder = H264Encoder::default();
        let frame = CaptureFrame::metadata_only(7, 1280, 720, 123_456);

        let encoded = encoder.encode(frame, 9).unwrap().unwrap();

        assert_eq!(encoded.room_stream_id, 9);
        assert_eq!(encoded.frame_id, 7);
        assert_eq!(encoded.sender_capture_time_micros, 123_456);
        assert_eq!(encoded.codec, CodecId::H264);
        assert!(!encoded.bytes.is_empty());
    }

    #[test]
    fn synthetic_encoder_uses_configured_payload_size() {
        let mut encoder = H264Encoder::default();
        encoder.config.synthetic_payload_bytes = 2048;

        let encoded = encoder
            .encode(CaptureFrame::metadata_only(7, 1280, 720, 123_456), 9)
            .unwrap()
            .unwrap();

        assert_eq!(encoded.bytes.len(), 2048);
    }

    #[test]
    fn keyframe_request_affects_next_frame_only() {
        let mut encoder = H264Encoder::default();
        encoder.request_keyframe();

        let first = encoder
            .encode(CaptureFrame::metadata_only(2, 1280, 720, 1), 9)
            .unwrap()
            .unwrap();
        let second = encoder
            .encode(CaptureFrame::metadata_only(3, 1280, 720, 2), 9)
            .unwrap()
            .unwrap();

        assert!(first.is_keyframe);
        assert!(!second.is_keyframe);
    }
}
