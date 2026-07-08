use super::RelayEndpoint;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuicClientConfig {
    pub relay: RelayEndpoint,
    pub max_datagram_payload: usize,
}

impl QuicClientConfig {
    pub fn new(relay: RelayEndpoint) -> Self {
        Self {
            relay,
            max_datagram_payload: teamview_protocol::packet::DEFAULT_DATAGRAM_PAYLOAD_TARGET,
        }
    }
}
