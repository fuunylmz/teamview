pub mod h264;

use std::collections::BTreeMap;

use bytes::Bytes;
use teamview_protocol::{
    frame::{EncodedFrame, reassemble_frame},
    packet::MediaPacket,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    pub frame_id: u32,
    pub width: u32,
    pub height: u32,
    pub pixel_format: DecodedPixelFormat,
    pub pixels: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedPixelFormat {
    Bgra8,
}

pub trait VideoDecoder {
    fn decode(&mut self, encoded: &[u8]) -> anyhow::Result<Option<DecodedFrame>>;
}

const MAX_PENDING_FRAMES: usize = 64;
const MAX_FRAME_AGE_FRAMES: u32 = 64;

#[derive(Debug, Default)]
pub struct FrameReassemblyBuffer {
    pending: BTreeMap<u32, PendingFrame>,
    max_pending_frames: usize,
    max_frame_age_frames: u32,
}

impl FrameReassemblyBuffer {
    pub fn new() -> Self {
        Self::with_limits(MAX_PENDING_FRAMES, MAX_FRAME_AGE_FRAMES)
    }

    pub fn with_limits(max_pending_frames: usize, max_frame_age_frames: u32) -> Self {
        Self {
            pending: BTreeMap::new(),
            max_pending_frames: max_pending_frames.max(1),
            max_frame_age_frames: max_frame_age_frames.max(1),
        }
    }

    pub fn push(&mut self, packet: MediaPacket) -> anyhow::Result<Option<EncodedFrame>> {
        self.push_with_stats(packet).map(|outcome| outcome.frame)
    }

    pub fn push_with_stats(&mut self, packet: MediaPacket) -> anyhow::Result<ReassemblyOutcome> {
        let frame_id = packet.header.frame_id;
        let fragment_count = packet.header.fragment_count as usize;
        let fragment_index = packet.header.fragment_index;
        let mut dropped_frames = self.evict_stale_frames(frame_id);
        while !self.pending.contains_key(&frame_id) && self.pending.len() >= self.max_pending_frames
        {
            let Some(oldest_frame_id) = self.pending.keys().next().copied() else {
                break;
            };
            self.pending.remove(&oldest_frame_id);
            dropped_frames += 1;
        }
        let pending = self.pending.entry(frame_id).or_default();
        if pending
            .packets
            .iter()
            .any(|pending| pending.header.fragment_index == fragment_index)
        {
            anyhow::bail!("duplicate media fragment {fragment_index} for frame {frame_id}");
        }
        pending.packets.push(packet);
        if pending.packets.len() != fragment_count {
            return Ok(ReassemblyOutcome {
                frame: None,
                dropped_frames,
            });
        }

        let packets = self.pending.remove(&frame_id).unwrap().packets;
        let frame = reassemble_frame(packets).map_err(anyhow::Error::from)?;
        Ok(ReassemblyOutcome {
            frame: Some(frame),
            dropped_frames,
        })
    }

    pub fn pending_frames(&self) -> usize {
        self.pending.len()
    }

    pub fn estimated_jitter_ms(&self, frame_interval_ms: u16) -> u16 {
        (self.pending.len() as u16).saturating_mul(frame_interval_ms)
    }

    fn evict_stale_frames(&mut self, newest_frame_id: u32) -> u64 {
        let stale_frame_ids = self
            .pending
            .keys()
            .copied()
            .filter(|frame_id| {
                frame_is_older_than_window(*frame_id, newest_frame_id, self.max_frame_age_frames)
            })
            .collect::<Vec<_>>();
        let dropped_frames = stale_frame_ids.len() as u64;
        for frame_id in stale_frame_ids {
            self.pending.remove(&frame_id);
        }
        dropped_frames
    }
}

#[derive(Debug, Default, Clone)]
struct PendingFrame {
    packets: Vec<MediaPacket>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReassemblyOutcome {
    pub frame: Option<EncodedFrame>,
    pub dropped_frames: u64,
}

fn frame_is_older_than_window(
    frame_id: u32,
    newest_frame_id: u32,
    max_frame_age_frames: u32,
) -> bool {
    let distance = newest_frame_id.wrapping_sub(frame_id);
    distance > max_frame_age_frames && distance < u32::MAX / 2
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use teamview_protocol::{
        codec::CodecId,
        frame::{EncodedFrame, packetize_frame},
    };

    use super::*;

    #[test]
    fn reassembly_buffer_outputs_complete_frame() {
        let frame = sample_frame();
        let packets = packetize_frame(&frame, 1, 8).unwrap();
        let mut buffer = FrameReassemblyBuffer::new();
        let mut reassembled = None;

        for packet in packets {
            reassembled = buffer.push(packet).unwrap();
        }

        assert_eq!(reassembled, Some(frame));
        assert_eq!(buffer.pending_frames(), 0);
    }

    #[test]
    fn reassembly_buffer_waits_for_missing_fragment() {
        let frame = sample_frame();
        let mut packets = packetize_frame(&frame, 1, 8).unwrap();
        packets.pop();
        let mut buffer = FrameReassemblyBuffer::new();

        for packet in packets {
            assert_eq!(buffer.push(packet).unwrap(), None);
        }

        assert_eq!(buffer.pending_frames(), 1);
    }

    #[test]
    fn reassembly_buffer_rejects_duplicate_fragment() {
        let frame = sample_frame();
        let packets = packetize_frame(&frame, 1, 8).unwrap();
        let mut buffer = FrameReassemblyBuffer::new();

        buffer.push(packets[0].clone()).unwrap();
        assert!(buffer.push(packets[0].clone()).is_err());
    }

    #[test]
    fn reassembly_buffer_evicts_old_pending_frames() {
        let mut buffer = FrameReassemblyBuffer::new();
        for frame_id in 1..=MAX_PENDING_FRAMES as u32 + 1 {
            let mut packets = packetize_frame(&sample_frame_with_id(frame_id), 1, 8).unwrap();
            packets.pop();
            buffer.push(packets.remove(0)).unwrap();
        }

        assert_eq!(buffer.pending_frames(), MAX_PENDING_FRAMES);
    }

    #[test]
    fn reassembly_buffer_reports_capacity_drops() {
        let mut buffer = FrameReassemblyBuffer::with_limits(1, 64);
        let mut first_packets = packetize_frame(&sample_frame_with_id(1), 1, 8).unwrap();
        first_packets.pop();
        assert_eq!(
            buffer.push_with_stats(first_packets.remove(0)).unwrap(),
            ReassemblyOutcome {
                frame: None,
                dropped_frames: 0,
            }
        );

        let mut second_packets = packetize_frame(&sample_frame_with_id(2), 10, 8).unwrap();
        second_packets.pop();
        assert_eq!(
            buffer.push_with_stats(second_packets.remove(0)).unwrap(),
            ReassemblyOutcome {
                frame: None,
                dropped_frames: 1,
            }
        );
    }

    #[test]
    fn reassembly_buffer_drops_stale_incomplete_frames() {
        let mut buffer = FrameReassemblyBuffer::with_limits(64, 2);
        let mut old_packets = packetize_frame(&sample_frame_with_id(1), 1, 8).unwrap();
        old_packets.pop();
        buffer.push_with_stats(old_packets.remove(0)).unwrap();

        let mut new_packets = packetize_frame(&sample_frame_with_id(4), 20, 8).unwrap();
        new_packets.pop();
        let outcome = buffer.push_with_stats(new_packets.remove(0)).unwrap();

        assert_eq!(outcome.dropped_frames, 1);
        assert_eq!(buffer.pending_frames(), 1);
    }

    #[test]
    fn jitter_estimate_scales_with_pending_frames() {
        let mut buffer = FrameReassemblyBuffer::new();
        let mut packets = packetize_frame(&sample_frame_with_id(1), 1, 8).unwrap();
        packets.pop();
        buffer.push(packets.remove(0)).unwrap();

        assert_eq!(buffer.estimated_jitter_ms(33), 33);
    }

    fn sample_frame() -> EncodedFrame {
        sample_frame_with_id(7)
    }

    fn sample_frame_with_id(frame_id: u32) -> EncodedFrame {
        EncodedFrame {
            room_stream_id: 9,
            frame_id,
            media_timestamp: 21_000,
            sender_capture_time_micros: 1_234,
            codec: CodecId::H264,
            is_keyframe: true,
            bytes: Bytes::from_static(b"synthetic-frame-payload"),
        }
    }
}
