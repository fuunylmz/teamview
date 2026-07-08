#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientRole {
    Broadcaster,
    Viewer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientApp {
    pub role: ClientRole,
}

impl ClientApp {
    pub fn new(role: ClientRole) -> Self {
        Self { role }
    }
}
