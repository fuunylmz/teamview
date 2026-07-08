#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub max_datagram_payload: usize,
    pub viewer_queue_budget_ms: u16,
}

impl ServerConfig {
    pub fn new(listen_addr: String) -> Self {
        Self {
            listen_addr,
            max_datagram_payload: teamview_protocol::packet::DEFAULT_DATAGRAM_PAYLOAD_TARGET,
            viewer_queue_budget_ms: 100,
        }
    }
}
