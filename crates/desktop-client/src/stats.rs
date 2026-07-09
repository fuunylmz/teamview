use teamview_protocol::{
    control::{RoomId, StreamId, ViewerStatsReport},
    packet::MediaPacket,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClientPipelineStats {
    pub capture_queue_len: u8,
    pub encode_queue_len: u8,
    pub network_queue_len: u16,
}

impl ClientPipelineStats {
    pub fn is_accumulating_latency(self) -> bool {
        self.capture_queue_len > 1 || self.encode_queue_len > 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClientMediaStats {
    pub received_packets: u64,
    pub lost_packets: u64,
    pub decoded_frames: u64,
    pub dropped_frames: u64,
    pub jitter_buffer_ms: u16,
    pub estimated_latency_ms: u16,
    last_sequence_number: Option<u32>,
}

impl ClientMediaStats {
    pub fn record_packet(&mut self, packet: &MediaPacket) {
        self.received_packets = self.received_packets.saturating_add(1);
        let sequence_number = packet.header.sequence_number;
        if let Some(previous) = self.last_sequence_number {
            let distance = sequence_number.wrapping_sub(previous);
            if distance > 0 && distance < u32::MAX / 2 {
                self.lost_packets = self.lost_packets.saturating_add((distance - 1) as u64);
                self.last_sequence_number = Some(sequence_number);
            }
        } else {
            self.last_sequence_number = Some(sequence_number);
        }
    }

    pub fn record_decoded_frame(&mut self) {
        self.decoded_frames = self.decoded_frames.saturating_add(1);
    }

    pub fn record_dropped_frame(&mut self) {
        self.dropped_frames = self.dropped_frames.saturating_add(1);
    }

    pub fn record_dropped_frames(&mut self, dropped_frames: u64) {
        self.dropped_frames = self.dropped_frames.saturating_add(dropped_frames);
    }

    pub fn to_viewer_report(self, room_id: RoomId, stream_id: StreamId) -> ViewerStatsReport {
        ViewerStatsReport {
            room_id,
            stream_id,
            received_packets: self.received_packets,
            lost_packets: self.lost_packets,
            decoded_frames: self.decoded_frames,
            dropped_frames: self.dropped_frames,
            jitter_buffer_ms: self.jitter_buffer_ms,
            estimated_latency_ms: self.estimated_latency_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use teamview_protocol::{
        codec::CodecId,
        packet::{MediaPacketHeader, PacketType},
    };

    use super::*;

    #[test]
    fn media_stats_detects_sequence_gaps() {
        let mut stats = ClientMediaStats::default();

        stats.record_packet(&packet_with_sequence(10));
        stats.record_packet(&packet_with_sequence(11));
        stats.record_packet(&packet_with_sequence(14));

        assert_eq!(stats.received_packets, 3);
        assert_eq!(stats.lost_packets, 2);
    }

    #[test]
    fn media_stats_builds_viewer_report() {
        let mut stats = ClientMediaStats::default();
        stats.record_packet(&packet_with_sequence(1));
        stats.record_decoded_frame();
        stats.record_dropped_frame();
        stats.jitter_buffer_ms = 42;
        stats.estimated_latency_ms = 88;

        let report = stats.to_viewer_report(7, 9);

        assert_eq!(report.room_id, 7);
        assert_eq!(report.stream_id, 9);
        assert_eq!(report.received_packets, 1);
        assert_eq!(report.decoded_frames, 1);
        assert_eq!(report.dropped_frames, 1);
        assert_eq!(report.jitter_buffer_ms, 42);
        assert_eq!(report.estimated_latency_ms, 88);
    }

    fn packet_with_sequence(sequence_number: u32) -> MediaPacket {
        let payload = Bytes::from_static(b"packet");
        MediaPacket {
            header: MediaPacketHeader::new(
                PacketType::Video,
                CodecId::H264,
                9,
                sequence_number,
                payload.len() as u16,
            ),
            payload,
        }
    }
}
