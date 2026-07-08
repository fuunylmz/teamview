use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct QueueStats {
    pub queued_packets: u32,
    pub queued_media_ms: u16,
    pub dropped_packets: u64,
    pub dropped_frames: u64,
}

impl QueueStats {
    pub fn is_over_budget(self, max_media_ms: u16) -> bool {
        self.queued_media_ms > max_media_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TransportStats {
    pub rtt_ms: u16,
    pub send_bitrate_bps: u32,
    pub receive_bitrate_bps: u32,
    pub lost_packets: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PipelineLatencyStats {
    pub capture_ms: u16,
    pub encode_ms: u16,
    pub server_route_ms: u16,
    pub jitter_buffer_ms: u16,
    pub decode_ms: u16,
    pub render_ms: u16,
}

impl PipelineLatencyStats {
    pub fn estimated_total_ms(self) -> u16 {
        self.capture_ms
            .saturating_add(self.encode_ms)
            .saturating_add(self.server_route_ms)
            .saturating_add(self.jitter_buffer_ms)
            .saturating_add(self.decode_ms)
            .saturating_add(self.render_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_budget_detects_slow_viewers() {
        let stats = QueueStats {
            queued_packets: 12,
            queued_media_ms: 151,
            dropped_packets: 3,
            dropped_frames: 1,
        };

        assert!(stats.is_over_budget(150));
    }

    #[test]
    fn latency_total_saturates() {
        let stats = PipelineLatencyStats {
            capture_ms: 10,
            encode_ms: 12,
            server_route_ms: 3,
            jitter_buffer_ms: 60,
            decode_ms: 8,
            render_ms: 10,
        };

        assert_eq!(stats.estimated_total_ms(), 103);
    }
}
