use bytes::Bytes;

use super::{DecodedFrame, DecodedPixelFormat, VideoDecoder};

const SYNTHETIC_SPS_MAGIC: &[u8; 4] = b"TVS1";
const SYNTHETIC_FRAME_MAGIC: &[u8; 4] = b"TVF1";
const NAL_NON_IDR_SLICE: u8 = 1;
const NAL_IDR_SLICE: u8 = 5;
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;
const MAX_SYNTHETIC_DECODED_PIXELS: u64 = 3840 * 2160;

#[derive(Debug, Default)]
pub struct H264Decoder {
    config: Option<SyntheticStreamConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyntheticStreamConfig {
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyntheticFrameInfo {
    frame_id: u32,
    width: u32,
    height: u32,
}

impl VideoDecoder for H264Decoder {
    fn decode(&mut self, encoded: &[u8]) -> anyhow::Result<Option<DecodedFrame>> {
        if encoded.is_empty() {
            return Ok(None);
        }

        let mut saw_sps = false;
        let mut saw_pps = false;
        let mut saw_idr = false;
        let mut saw_slice = false;
        let mut frame_info = None;

        for nal in annex_b_nals(encoded) {
            let Some((&header, payload)) = nal.split_first() else {
                continue;
            };
            match header & 0x1f {
                NAL_SPS => {
                    saw_sps = true;
                    if let Some(config) = parse_synthetic_sps(payload) {
                        self.config = Some(config);
                    }
                }
                NAL_PPS => saw_pps = true,
                NAL_IDR_SLICE => {
                    saw_idr = true;
                    saw_slice = true;
                    frame_info = parse_synthetic_frame(payload).or(frame_info);
                }
                NAL_NON_IDR_SLICE => {
                    saw_slice = true;
                    frame_info = parse_synthetic_frame(payload).or(frame_info);
                }
                _ => {}
            }
        }

        if !saw_slice {
            return Ok(None);
        }
        if saw_idr && (!saw_sps || !saw_pps || self.config.is_none()) {
            return Ok(None);
        }

        let Some(config) = self.config else {
            return Ok(None);
        };
        let frame_info = frame_info.unwrap_or(SyntheticFrameInfo {
            frame_id: 0,
            width: config.width,
            height: config.height,
        });
        let width = frame_info.width.max(1);
        let height = frame_info.height.max(1);
        if width as u64 * height as u64 > MAX_SYNTHETIC_DECODED_PIXELS {
            anyhow::bail!("synthetic decoded frame exceeds maximum preview size");
        }
        Ok(Some(DecodedFrame {
            frame_id: frame_info.frame_id,
            width,
            height,
            pixel_format: DecodedPixelFormat::Bgra8,
            pixels: synthetic_bgra_pixels(width, height, frame_info.frame_id, saw_idr),
        }))
    }
}

fn parse_synthetic_sps(payload: &[u8]) -> Option<SyntheticStreamConfig> {
    let bytes = payload.get(..14)?;
    if &bytes[..4] != SYNTHETIC_SPS_MAGIC {
        return None;
    }
    let width = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    if width == 0 || height == 0 {
        return None;
    }
    Some(SyntheticStreamConfig { width, height })
}

fn parse_synthetic_frame(payload: &[u8]) -> Option<SyntheticFrameInfo> {
    let bytes = payload.get(..16)?;
    if &bytes[..4] != SYNTHETIC_FRAME_MAGIC {
        return None;
    }
    let frame_id = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let width = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
    if width == 0 || height == 0 {
        return None;
    }
    Some(SyntheticFrameInfo {
        frame_id,
        width,
        height,
    })
}

fn synthetic_bgra_pixels(width: u32, height: u32, frame_id: u32, is_keyframe: bool) -> Bytes {
    let pixel_count = (width as usize).saturating_mul(height as usize);
    let mut pixels = Vec::with_capacity(pixel_count.saturating_mul(4));
    let keyframe_bias = if is_keyframe { 0x40 } else { 0x10 };
    for y in 0..height {
        for x in 0..width {
            let pattern = ((x / 32) ^ (y / 32) ^ frame_id) as u8;
            pixels.push(pattern.wrapping_mul(3));
            pixels.push(pattern.wrapping_mul(5).wrapping_add(keyframe_bias));
            pixels.push(pattern.wrapping_mul(7).wrapping_add(frame_id as u8));
            pixels.push(0xff);
        }
    }
    Bytes::from(pixels)
}

fn annex_b_nals(encoded: &[u8]) -> AnnexBNals<'_> {
    AnnexBNals {
        bytes: encoded,
        offset: 0,
    }
}

#[derive(Debug, Clone)]
struct AnnexBNals<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for AnnexBNals<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let start = find_start_code(self.bytes, self.offset)?;
        let nal_start = start + start_code_len(&self.bytes[start..])?;
        let next_start = find_start_code(self.bytes, nal_start).unwrap_or(self.bytes.len());
        self.offset = next_start;
        Some(&self.bytes[nal_start..next_start])
    }
}

fn find_start_code(bytes: &[u8], offset: usize) -> Option<usize> {
    let mut index = offset;
    while index + 3 <= bytes.len() {
        if bytes[index..].starts_with(&[0, 0, 1])
            || (index + 4 <= bytes.len() && bytes[index..].starts_with(&[0, 0, 0, 1]))
        {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn start_code_len(bytes: &[u8]) -> Option<usize> {
    if bytes.starts_with(&[0, 0, 0, 1]) {
        Some(4)
    } else if bytes.starts_with(&[0, 0, 1]) {
        Some(3)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        capture::CaptureFrame,
        encode::{VideoEncoder, h264::H264Encoder},
    };

    use super::*;

    #[test]
    fn synthetic_decoder_outputs_frame_from_keyframe_metadata() {
        let mut decoder = H264Decoder::default();
        let encoded = encode_frame(1, 1920, 1080).bytes;

        let decoded = decoder.decode(&encoded).unwrap();

        assert_eq!(
            decoded,
            Some(DecodedFrame {
                frame_id: 1,
                width: 1920,
                height: 1080,
                pixel_format: DecodedPixelFormat::Bgra8,
                pixels: synthetic_bgra_pixels(1920, 1080, 1, true),
            })
        );
    }

    #[test]
    fn synthetic_decoder_waits_on_empty_input() {
        let mut decoder = H264Decoder::default();

        assert_eq!(decoder.decode(b"").unwrap(), None);
    }

    #[test]
    fn synthetic_decoder_waits_for_keyframe_before_delta_frames() {
        let mut decoder = H264Decoder::default();
        let encoded = encode_frame(2, 1280, 720).bytes;

        assert_eq!(decoder.decode(&encoded).unwrap(), None);
    }

    #[test]
    fn synthetic_decoder_decodes_delta_after_keyframe() {
        let mut decoder = H264Decoder::default();
        let keyframe = encode_frame(1, 1280, 720).bytes;
        let delta = encode_frame(2, 1280, 720).bytes;

        assert!(decoder.decode(&keyframe).unwrap().is_some());
        assert_eq!(
            decoder.decode(&delta).unwrap(),
            Some(DecodedFrame {
                frame_id: 2,
                width: 1280,
                height: 720,
                pixel_format: DecodedPixelFormat::Bgra8,
                pixels: synthetic_bgra_pixels(1280, 720, 2, false),
            })
        );
    }

    #[test]
    fn synthetic_decoder_outputs_bgra_pixels() {
        let mut decoder = H264Decoder::default();
        let encoded = encode_frame(1, 64, 36).bytes;

        let decoded = decoder.decode(&encoded).unwrap().unwrap();

        assert_eq!(decoded.frame_id, 1);
        assert_eq!(decoded.pixel_format, DecodedPixelFormat::Bgra8);
        assert_eq!(decoded.pixels.len(), 64 * 36 * 4);
        assert_eq!(decoded.pixels[3], 0xff);
    }

    #[test]
    fn annex_b_parser_accepts_three_byte_start_codes() {
        let bytes = [0, 0, 1, 0x67, 1, 0, 0, 1, 0x68, 2];
        let nals = annex_b_nals(&bytes).collect::<Vec<_>>();

        assert_eq!(nals, vec![&[0x67, 1][..], &[0x68, 2][..]]);
    }

    fn encode_frame(
        frame_id: u64,
        width: u32,
        height: u32,
    ) -> teamview_protocol::frame::EncodedFrame {
        let mut encoder = H264Encoder::default();
        encoder
            .encode(CaptureFrame::metadata_only(frame_id, width, height, 1), 9)
            .unwrap()
            .unwrap()
    }
}
