#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub max_datagram_payload: usize,
    pub viewer_queue_budget_ms: u16,
    pub access_token: Option<String>,
}

impl ServerConfig {
    pub fn new(listen_addr: String) -> Self {
        Self {
            listen_addr,
            max_datagram_payload: teamview_protocol::packet::DEFAULT_DATAGRAM_PAYLOAD_TARGET,
            viewer_queue_budget_ms: 100,
            access_token: None,
        }
    }

    pub fn with_access_token(mut self, access_token: Option<String>) -> Self {
        self.access_token = access_token;
        self
    }

    pub fn with_max_datagram_payload(mut self, max_datagram_payload: usize) -> Self {
        self.max_datagram_payload = max_datagram_payload.max(1);
        self
    }
}
