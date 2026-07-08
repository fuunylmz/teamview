pub mod quic;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayEndpoint {
    pub addr: String,
}

impl RelayEndpoint {
    pub fn new(addr: impl Into<String>) -> Self {
        Self { addr: addr.into() }
    }
}
