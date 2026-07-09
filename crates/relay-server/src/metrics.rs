use std::time::{Duration, SystemTime, UNIX_EPOCH};

use teamview_protocol::control::{RoomId, StreamId, StreamMetricsSnapshot};

const ROUTE_SAMPLE_CAPACITY: usize = 64;

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
    server_route_samples_ms: RouteLatencySamples,
}

impl StreamForwardingMetrics {
    pub fn record_forwarding(
        &mut self,
        ingress_bytes: usize,
        queued_packets: u32,
        dropped_packets: u32,
        received_at_micros: u64,
        server_route_ms: u16,
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
        self.server_route_samples_ms.push(server_route_ms);
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
            egress_queue_packets: 0,
            egress_queue_media_ms: 0,
            subscriber_count,
            last_ingress_time_micros: self.last_ingress_time_micros,
            server_route_ms_p50: self.server_route_samples_ms.percentile(50),
            server_route_ms_p95: self.server_route_samples_ms.percentile(95),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RouteLatencySamples {
    values: [u16; ROUTE_SAMPLE_CAPACITY],
    len: usize,
    next: usize,
}

impl Default for RouteLatencySamples {
    fn default() -> Self {
        Self {
            values: [0; ROUTE_SAMPLE_CAPACITY],
            len: 0,
            next: 0,
        }
    }
}

impl RouteLatencySamples {
    fn push(&mut self, value: u16) {
        self.values[self.next] = value;
        self.next = (self.next + 1) % ROUTE_SAMPLE_CAPACITY;
        self.len = self.len.saturating_add(1).min(ROUTE_SAMPLE_CAPACITY);
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

pub fn micros_delta_to_millis(start_micros: u64, end_micros: u64) -> u16 {
    end_micros
        .saturating_sub(start_micros)
        .saturating_div(1_000)
        .min(u16::MAX as u64) as u16
}

pub fn unix_time_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_metrics_report_route_latency_percentiles() {
        let mut metrics = StreamForwardingMetrics::default();
        metrics.record_forwarding(100, 1, 0, 1_000_000, 2);
        metrics.record_forwarding(100, 1, 0, 1_020_000, 8);
        metrics.record_forwarding(100, 1, 0, 1_040_000, 5);

        let snapshot = metrics.snapshot(7, 9, 1);

        assert_eq!(snapshot.server_route_ms_p50, 5);
        assert_eq!(snapshot.server_route_ms_p95, 8);
    }

    #[test]
    fn micros_delta_to_millis_saturates_and_ignores_clock_reversal() {
        assert_eq!(micros_delta_to_millis(1_000, 3_500), 2);
        assert_eq!(micros_delta_to_millis(3_500, 1_000), 0);
        assert_eq!(micros_delta_to_millis(0, u64::MAX), u16::MAX);
    }
}
