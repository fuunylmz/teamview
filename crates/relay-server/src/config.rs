use crate::control::{
    ControlLimits, DEFAULT_MAX_PARTICIPANTS_PER_ROOM, DEFAULT_MAX_ROOMS,
    DEFAULT_MAX_STREAMS_PER_ROOM,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub max_datagram_payload: usize,
    pub viewer_queue_budget_ms: u16,
    pub access_token: Option<String>,
    pub control_limits: ControlLimits,
}

impl ServerConfig {
    pub fn new(listen_addr: String) -> Self {
        Self {
            listen_addr,
            max_datagram_payload: teamview_protocol::packet::DEFAULT_DATAGRAM_PAYLOAD_TARGET,
            viewer_queue_budget_ms: 100,
            access_token: None,
            control_limits: ControlLimits {
                max_rooms: DEFAULT_MAX_ROOMS,
                max_participants_per_room: DEFAULT_MAX_PARTICIPANTS_PER_ROOM,
                max_streams_per_room: DEFAULT_MAX_STREAMS_PER_ROOM,
            },
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

    pub fn with_control_limits(mut self, control_limits: ControlLimits) -> Self {
        self.control_limits = control_limits.sanitized();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_limits_are_sanitized() {
        let config =
            ServerConfig::new("127.0.0.1:0".to_owned()).with_control_limits(ControlLimits {
                max_rooms: 0,
                max_participants_per_room: 0,
                max_streams_per_room: 0,
            });

        assert_eq!(
            config.control_limits,
            ControlLimits {
                max_rooms: 1,
                max_participants_per_room: 1,
                max_streams_per_room: 1,
            }
        );
    }
}
