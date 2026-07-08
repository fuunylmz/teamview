#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RelayMetrics {
    pub active_rooms: u64,
    pub active_sessions: u64,
    pub forwarded_packets: u64,
    pub dropped_packets: u64,
}
