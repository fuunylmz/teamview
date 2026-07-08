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
