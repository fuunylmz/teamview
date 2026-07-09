use std::time::{Duration, SystemTime, UNIX_EPOCH};

use teamview_protocol::control::{RoomId, StreamId, StreamMetricsSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RelayMetrics {
    pub active_rooms: u64,
    pub active_sessions: u64,
    pub forwarded_packets: u64,
    pub dropped_packets: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StreamForwardingMetrics {
    pub ingress_packets: u64,
    pub ingress_bytes: u64,
    pub egress_queued_packets: u64,
    pub egress_dropped_packets: u64,
    pub last_ingress_time_micros: u64,
}

impl StreamForwardingMetrics {
    pub fn record_forwarding(
        &mut self,
        ingress_bytes: usize,
        queued_packets: u32,
        dropped_packets: u32,
        received_at_micros: u64,
    ) {
        self.ingress_packets = self.ingress_packets.saturating_add(1);
        self.ingress_bytes = self.ingress_bytes.saturating_add(ingress_bytes as u64);
        self.egress_queued_packets = self
            .egress_queued_packets
            .saturating_add(queued_packets as u64);
        self.egress_dropped_packets = self
            .egress_dropped_packets
            .saturating_add(dropped_packets as u64);
        self.last_ingress_time_micros = received_at_micros;
    }

    pub fn snapshot(
        self,
        room_id: RoomId,
        stream_id: StreamId,
        subscriber_count: u32,
    ) -> StreamMetricsSnapshot {
        StreamMetricsSnapshot {
            room_id,
            stream_id,
            ingress_packets: self.ingress_packets,
            ingress_bytes: self.ingress_bytes,
            egress_queued_packets: self.egress_queued_packets,
            egress_dropped_packets: self.egress_dropped_packets,
            subscriber_count,
            last_ingress_time_micros: self.last_ingress_time_micros,
        }
    }
}

pub fn unix_time_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros()
        .min(u64::MAX as u128) as u64
}
