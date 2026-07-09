use bytes::Bytes;
use teamview_protocol::{codec::CodecId, frame::EncodedFrame};

use crate::capture::{CaptureFrame, CaptureFrameStorage, CapturePixelFormat};

use super::VideoEncoder;

const ANNEX_B_START_CODE: &[u8; 4] = &[0x00, 0x00, 0x00, 0x01];
const SYNTHETIC_SPS_MAGIC: &[u8; 4] = b"TVS1";
const SYNTHETIC_PPS_MAGIC: &[u8; 4] = b"TVP1";
const SYNTHETIC_FRAME_MAGIC: &[u8; 4] = b"TVF1";
const SYNTHETIC_PREVIEW_MAGIC: &[u8; 4] = b"TVB1";
const MAX_SYNTHETIC_PREVIEW_WIDTH: u32 = 160;
const MAX_SYNTHETIC_PREVIEW_HEIGHT: u32 = 90;
const NAL_SPS: u8 = 0x67;
const NAL_PPS: u8 = 0x68;
const NAL_IDR_SLICE: u8 = 0x65;
const NAL_NON_IDR_SLICE: u8 = 0x41;

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
        if is_keyframe {
            append_nal(
                &mut bytes,
                NAL_SPS,
                &synthetic_sps_payload(frame.width, frame.height, self.config.frames_per_second),
            );
            append_nal(&mut bytes, NAL_PPS, SYNTHETIC_PPS_MAGIC);
            append_nal(&mut bytes, NAL_IDR_SLICE, &synthetic_frame_payload(&frame));
        } else {
            append_nal(
                &mut bytes,
                NAL_NON_IDR_SLICE,
                &synthetic_frame_payload(&frame),
            );
        }
        while bytes.len() < self.config.synthetic_payload_bytes {
            bytes.push(
                frame
                    .frame_id
                    .wrapping_add(bytes.len() as u64)
                    .to_le_bytes()[0],
            );
        }

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

fn append_nal(bytes: &mut Vec<u8>, nal_header: u8, payload: &[u8]) {
    bytes.extend_from_slice(ANNEX_B_START_CODE);
    bytes.push(nal_header);
    bytes.extend_from_slice(&annex_b_escape_payload(payload));
}

fn annex_b_escape_payload(payload: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(payload.len());
    let mut consecutive_zeros = 0_u8;
    for &byte in payload {
        if consecutive_zeros >= 2 && byte <= 3 {
            escaped.push(0x03);
            consecutive_zeros = 0;
        }
        escaped.push(byte);
        if byte == 0 {
            consecutive_zeros = consecutive_zeros.saturating_add(1);
        } else {
            consecutive_zeros = 0;
        }
    }
    escaped
}

fn synthetic_sps_payload(width: u32, height: u32, frames_per_second: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(14);
    payload.extend_from_slice(SYNTHETIC_SPS_MAGIC);
    payload.extend_from_slice(&width.to_le_bytes());
    payload.extend_from_slice(&height.to_le_bytes());
    payload.extend_from_slice(&frames_per_second.to_le_bytes());
    payload
}

fn synthetic_frame_payload(frame: &CaptureFrame) -> Vec<u8> {
    let mut payload = Vec::with_capacity(16);
    payload.extend_from_slice(SYNTHETIC_FRAME_MAGIC);
    payload.extend_from_slice(&(frame.frame_id as u32).to_le_bytes());
    payload.extend_from_slice(&frame.width.to_le_bytes());
    payload.extend_from_slice(&frame.height.to_le_bytes());
    if let Some(preview) = synthetic_preview_payload(frame) {
        payload.extend_from_slice(&preview);
    }
    payload
}

fn synthetic_preview_payload(frame: &CaptureFrame) -> Option<Vec<u8>> {
    let CaptureFrameStorage::CpuBytes(source_pixels) = &frame.storage else {
        return None;
    };
    if frame.format != CapturePixelFormat::Bgra8 {
        return None;
    }
    let expected_len = CaptureFrame::bgra_byte_len(frame.width, frame.height).ok()?;
    if source_pixels.len() != expected_len {
        return None;
    }

    let (preview_width, preview_height) = preview_dimensions(frame.width, frame.height);
    let preview_pixels = downsample_bgra_nearest(
        source_pixels,
        frame.width,
        frame.height,
        preview_width,
        preview_height,
    )?;
    let mut payload = Vec::with_capacity(16 + preview_pixels.len());
    payload.extend_from_slice(SYNTHETIC_PREVIEW_MAGIC);
    payload.extend_from_slice(&preview_width.to_le_bytes());
    payload.extend_from_slice(&preview_height.to_le_bytes());
    payload.extend_from_slice(&(preview_pixels.len() as u32).to_le_bytes());
    payload.extend_from_slice(&preview_pixels);
    Some(payload)
}

fn preview_dimensions(width: u32, height: u32) -> (u32, u32) {
    if width <= MAX_SYNTHETIC_PREVIEW_WIDTH && height <= MAX_SYNTHETIC_PREVIEW_HEIGHT {
        return (width.max(1), height.max(1));
    }

    let height_by_width =
        (height as u64 * MAX_SYNTHETIC_PREVIEW_WIDTH as u64 / width.max(1) as u64) as u32;
    if height_by_width <= MAX_SYNTHETIC_PREVIEW_HEIGHT {
        return (MAX_SYNTHETIC_PREVIEW_WIDTH, height_by_width.max(1));
    }

    let width_by_height =
        (width as u64 * MAX_SYNTHETIC_PREVIEW_HEIGHT as u64 / height.max(1) as u64) as u32;
    (width_by_height.max(1), MAX_SYNTHETIC_PREVIEW_HEIGHT)
}

fn downsample_bgra_nearest(
    source_pixels: &[u8],
    source_width: u32,
    source_height: u32,
    preview_width: u32,
    preview_height: u32,
) -> Option<Vec<u8>> {
    let preview_len = CaptureFrame::bgra_byte_len(preview_width, preview_height).ok()?;
    let mut preview = Vec::with_capacity(preview_len);
    for y in 0..preview_height {
        let source_y = y as u64 * source_height as u64 / preview_height as u64;
        for x in 0..preview_width {
            let source_x = x as u64 * source_width as u64 / preview_width as u64;
            let offset = ((source_y * source_width as u64 + source_x) * 4) as usize;
            preview.extend_from_slice(source_pixels.get(offset..offset + 4)?);
        }
    }
    Some(preview)
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
    fn synthetic_encoder_keeps_required_metadata_when_payload_size_is_tiny() {
        let mut encoder = H264Encoder::default();
        encoder.config.synthetic_payload_bytes = 8;

        let encoded = encoder
            .encode(CaptureFrame::metadata_only(1, 1280, 720, 123_456), 9)
            .unwrap()
            .unwrap();

        assert!(encoded.bytes.len() > 8);
        assert!(
            encoded
                .bytes
                .windows(ANNEX_B_START_CODE.len())
                .any(|window| window == ANNEX_B_START_CODE)
        );
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

    #[test]
    fn synthetic_encoder_embeds_cpu_bgra_preview() {
        let mut encoder = H264Encoder::default();
        let pixels = (0_u8..32).collect::<Vec<_>>();
        let frame = CaptureFrame::cpu_bgra(1, 4, 2, 123_456, pixels.clone()).unwrap();

        let encoded = encoder.encode(frame, 9).unwrap().unwrap();

        assert!(
            encoded
                .bytes
                .windows(SYNTHETIC_PREVIEW_MAGIC.len())
                .any(|window| window == SYNTHETIC_PREVIEW_MAGIC)
        );
        assert!(
            encoded
                .bytes
                .windows(pixels.len())
                .any(|window| window == pixels.as_slice())
        );
    }

    #[test]
    fn preview_dimensions_preserve_aspect_without_upscaling() {
        assert_eq!(preview_dimensions(4, 2), (4, 2));
        assert_eq!(preview_dimensions(1920, 1080), (160, 90));
        assert_eq!(preview_dimensions(1080, 1920), (50, 90));
    }
}
