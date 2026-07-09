use std::time::Duration;

use teamview_protocol::{
    control::{RoomId, StreamId, ViewerStatsReport},
    packet::MediaPacket,
};

const LATENCY_SAMPLE_CAPACITY: usize = 64;

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
pub struct BroadcasterTimingSnapshot {
    pub capture_ms_p50: u16,
    pub capture_ms_p95: u16,
    pub encode_ms_p50: u16,
    pub encode_ms_p95: u16,
    pub packetize_ms_p50: u16,
    pub packetize_ms_p95: u16,
    pub send_ms_p50: u16,
    pub send_ms_p95: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClientBroadcasterStats {
    capture_samples_ms: LatencySamples,
    encode_samples_ms: LatencySamples,
    packetize_samples_ms: LatencySamples,
    send_samples_ms: LatencySamples,
}

impl ClientBroadcasterStats {
    pub fn record_capture_duration(&mut self, duration: Duration) {
        self.capture_samples_ms.push(duration_to_millis(duration));
    }

    pub fn record_encode_duration(&mut self, duration: Duration) {
        self.encode_samples_ms.push(duration_to_millis(duration));
    }

    pub fn record_packetize_duration(&mut self, duration: Duration) {
        self.packetize_samples_ms.push(duration_to_millis(duration));
    }

    pub fn record_send_duration(&mut self, duration: Duration) {
        self.send_samples_ms.push(duration_to_millis(duration));
    }

    pub fn timing_snapshot(self) -> BroadcasterTimingSnapshot {
        BroadcasterTimingSnapshot {
            capture_ms_p50: self.capture_samples_ms.percentile(50),
            capture_ms_p95: self.capture_samples_ms.percentile(95),
            encode_ms_p50: self.encode_samples_ms.percentile(50),
            encode_ms_p95: self.encode_samples_ms.percentile(95),
            packetize_ms_p50: self.packetize_samples_ms.percentile(50),
            packetize_ms_p95: self.packetize_samples_ms.percentile(95),
            send_ms_p50: self.send_samples_ms.percentile(50),
            send_ms_p95: self.send_samples_ms.percentile(95),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClientMediaStats {
    pub received_packets: u64,
    pub lost_packets: u64,
    pub decoded_frames: u64,
    pub rendered_frames: u64,
    pub dropped_frames: u64,
    pub jitter_buffer_ms: u16,
    pub estimated_latency_ms: u16,
    last_sequence_number: Option<u32>,
    decode_samples_ms: LatencySamples,
    render_samples_ms: LatencySamples,
    first_render_time_micros: u64,
    last_render_time_micros: u64,
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

    pub fn record_decode_duration(&mut self, duration: Duration) {
        self.decode_samples_ms.push(duration_to_millis(duration));
    }

    pub fn record_render_duration(&mut self, duration: Duration, render_time_micros: u64) {
        self.rendered_frames = self.rendered_frames.saturating_add(1);
        self.render_samples_ms.push(duration_to_millis(duration));
        if self.first_render_time_micros == 0 {
            self.first_render_time_micros = render_time_micros;
        }
        self.last_render_time_micros = render_time_micros;
    }

    pub fn record_dropped_frame(&mut self) {
        self.dropped_frames = self.dropped_frames.saturating_add(1);
    }

    pub fn record_dropped_frames(&mut self, dropped_frames: u64) {
        self.dropped_frames = self.dropped_frames.saturating_add(dropped_frames);
    }

    pub fn record_estimated_latency(
        &mut self,
        sender_capture_time_micros: u64,
        receive_time_micros: u64,
    ) {
        if sender_capture_time_micros == 0 || receive_time_micros < sender_capture_time_micros {
            return;
        }
        let latency_ms = receive_time_micros
            .saturating_sub(sender_capture_time_micros)
            .saturating_div(1_000)
            .min(u16::MAX as u64) as u16;
        self.estimated_latency_ms = latency_ms;
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
            decode_ms_p50: self.decode_samples_ms.percentile(50),
            decode_ms_p95: self.decode_samples_ms.percentile(95),
            render_ms_p50: self.render_samples_ms.percentile(50),
            render_ms_p95: self.render_samples_ms.percentile(95),
            render_fps: self.render_fps(),
        }
    }

    pub fn render_fps(self) -> u16 {
        if self.rendered_frames < 2 || self.last_render_time_micros <= self.first_render_time_micros
        {
            return 0;
        }
        let elapsed_micros = self
            .last_render_time_micros
            .saturating_sub(self.first_render_time_micros);
        self.rendered_frames
            .saturating_sub(1)
            .saturating_mul(1_000_000)
            .checked_div(elapsed_micros)
            .unwrap_or_default()
            .min(u16::MAX as u64) as u16
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LatencySamples {
    values: [u16; LATENCY_SAMPLE_CAPACITY],
    len: usize,
    next: usize,
}

impl Default for LatencySamples {
    fn default() -> Self {
        Self {
            values: [0; LATENCY_SAMPLE_CAPACITY],
            len: 0,
            next: 0,
        }
    }
}

impl LatencySamples {
    fn push(&mut self, value: u16) {
        self.values[self.next] = value;
        self.next = (self.next + 1) % LATENCY_SAMPLE_CAPACITY;
        self.len = self.len.saturating_add(1).min(LATENCY_SAMPLE_CAPACITY);
    }

    fn percentile(self, percentile: u8) -> u16 {
        if self.len == 0 {
            return 0;
        }
        let mut samples = self.values[..self.len].to_vec();
        samples.sort_unstable();
        let index = ((self.len - 1) as u32)
            .saturating_mul(percentile.min(100) as u32)
            .div_ceil(100) as usize;
        samples[index.min(samples.len() - 1)]
    }
}

fn duration_to_millis(duration: Duration) -> u16 {
    duration.as_millis().min(u16::MAX as u128) as u16
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
        stats.record_decode_duration(Duration::from_millis(4));
        stats.record_decode_duration(Duration::from_millis(8));
        stats.record_render_duration(Duration::from_millis(3), 1_000_000);
        stats.record_render_duration(Duration::from_millis(5), 1_050_000);
        stats.record_dropped_frame();
        stats.jitter_buffer_ms = 42;
        stats.estimated_latency_ms = 88;

        let report = stats.to_viewer_report(7, 9);

        assert_eq!(report.room_id, 7);
        assert_eq!(report.stream_id, 9);
        assert_eq!(report.received_packets, 1);
        assert_eq!(report.decoded_frames, 1);
        assert_eq!(report.decode_ms_p50, 8);
        assert_eq!(report.decode_ms_p95, 8);
        assert_eq!(report.render_ms_p50, 5);
        assert_eq!(report.render_ms_p95, 5);
        assert_eq!(report.render_fps, 20);
        assert_eq!(report.dropped_frames, 1);
        assert_eq!(report.jitter_buffer_ms, 42);
        assert_eq!(report.estimated_latency_ms, 88);
    }

    #[test]
    fn media_stats_estimates_latency_from_capture_timestamp() {
        let mut stats = ClientMediaStats::default();

        stats.record_estimated_latency(1_000_000, 1_123_456);

        assert_eq!(stats.estimated_latency_ms, 123);
    }

    #[test]
    fn media_stats_ignores_missing_or_future_capture_timestamp() {
        let mut stats = ClientMediaStats {
            estimated_latency_ms: 42,
            ..Default::default()
        };

        stats.record_estimated_latency(0, 1_123_456);
        stats.record_estimated_latency(2_000_000, 1_123_456);

        assert_eq!(stats.estimated_latency_ms, 42);
    }

    #[test]
    fn broadcaster_stats_builds_timing_snapshot() {
        let mut stats = ClientBroadcasterStats::default();
        stats.record_capture_duration(Duration::from_millis(2));
        stats.record_capture_duration(Duration::from_millis(6));
        stats.record_encode_duration(Duration::from_millis(3));
        stats.record_encode_duration(Duration::from_millis(9));
        stats.record_packetize_duration(Duration::from_millis(1));
        stats.record_send_duration(Duration::from_millis(4));
        stats.record_send_duration(Duration::from_millis(5));

        let snapshot = stats.timing_snapshot();

        assert_eq!(snapshot.capture_ms_p50, 6);
        assert_eq!(snapshot.capture_ms_p95, 6);
        assert_eq!(snapshot.encode_ms_p50, 9);
        assert_eq!(snapshot.encode_ms_p95, 9);
        assert_eq!(snapshot.packetize_ms_p50, 1);
        assert_eq!(snapshot.packetize_ms_p95, 1);
        assert_eq!(snapshot.send_ms_p50, 5);
        assert_eq!(snapshot.send_ms_p95, 5);
    }

    #[test]
    fn broadcaster_stats_empty_snapshot_is_zero() {
        let snapshot = ClientBroadcasterStats::default().timing_snapshot();

        assert_eq!(snapshot, BroadcasterTimingSnapshot::default());
    }

    #[test]
    fn latency_samples_keep_recent_percentiles() {
        let mut samples = LatencySamples::default();
        samples.push(2);
        samples.push(10);
        samples.push(6);

        assert_eq!(samples.percentile(50), 6);
        assert_eq!(samples.percentile(95), 10);
    }

    #[test]
    fn render_fps_requires_two_rendered_frames() {
        let mut stats = ClientMediaStats::default();

        stats.record_render_duration(Duration::from_millis(1), 1_000_000);
        assert_eq!(stats.render_fps(), 0);

        stats.record_render_duration(Duration::from_millis(1), 1_100_000);
        assert_eq!(stats.render_fps(), 10);
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
