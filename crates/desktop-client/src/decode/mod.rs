pub mod h264;

use std::collections::BTreeMap;

use teamview_protocol::{
    frame::{EncodedFrame, reassemble_frame},
    packet::MediaPacket,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub render_time_micros: u64,
}

pub trait VideoDecoder {
    fn decode(&mut self, encoded: &[u8]) -> anyhow::Result<Option<DecodedFrame>>;
}

const MAX_PENDING_FRAMES: usize = 64;

#[derive(Debug, Default)]
pub struct FrameReassemblyBuffer {
    pending: BTreeMap<u32, Vec<MediaPacket>>,
}

impl FrameReassemblyBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, packet: MediaPacket) -> anyhow::Result<Option<EncodedFrame>> {
        let frame_id = packet.header.frame_id;
        let fragment_count = packet.header.fragment_count as usize;
        let fragment_index = packet.header.fragment_index;
        if !self.pending.contains_key(&frame_id)
            && self.pending.len() >= MAX_PENDING_FRAMES
            && let Some(oldest_frame_id) = self.pending.keys().next().copied()
        {
            self.pending.remove(&oldest_frame_id);
        }
        let packets = self.pending.entry(frame_id).or_default();
        if packets
            .iter()
            .any(|pending| pending.header.fragment_index == fragment_index)
        {
            anyhow::bail!("duplicate media fragment {fragment_index} for frame {frame_id}");
        }
        packets.push(packet);
        if packets.len() != fragment_count {
            return Ok(None);
        }

        let packets = self.pending.remove(&frame_id).unwrap();
        reassemble_frame(packets).map(Some).map_err(Into::into)
    }

    pub fn pending_frames(&self) -> usize {
        self.pending.len()
    }
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
