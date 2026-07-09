use std::collections::BTreeMap;

use anyhow::Context;
use quinn::Connection;
use teamview_protocol::{
    control::{MediaKind, UserId},
    packet::{MediaPacket, PacketType},
};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::control::ControlState;

#[derive(Debug, Default)]
pub struct MediaRelay {
    connections: BTreeMap<UserId, Connection>,
}

impl MediaRelay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, user_id: UserId, connection: Connection) {
        self.connections.insert(user_id, connection);
    }

    pub fn unregister(&mut self, user_id: UserId) {
        self.connections.remove(&user_id);
    }

    pub fn forward_media_packet(
        &self,
        state: &ControlState,
        publisher_id: UserId,
        packet: &MediaPacket,
    ) -> MediaForwardSummary {
        let stream_id = packet.header.room_stream_id;
        let Some(published) = state.published_stream(stream_id) else {
            return MediaForwardSummary {
                stream_id,
                delivered: 0,
                dropped: 1,
            };
        };
        if published.publisher_id != publisher_id
            || published.codec != packet.header.codec
            || media_kind_packet_type(published.media_kind) != packet.header.packet_type
        {
            return MediaForwardSummary {
                stream_id,
                delivered: 0,
                dropped: 1,
            };
        }

        let Ok(encoded) = packet.encode() else {
            return MediaForwardSummary {
                stream_id,
                delivered: 0,
                dropped: 1,
            };
        };

        let mut summary = MediaForwardSummary {
            stream_id,
            delivered: 0,
            dropped: 0,
        };
        for subscriber_id in state.subscribers_for_stream(stream_id) {
            if subscriber_id == publisher_id {
                continue;
            }
            let Some(connection) = self.connections.get(&subscriber_id) else {
                summary.dropped += 1;
                continue;
            };
            match connection.send_datagram(encoded.clone()) {
                Ok(()) => summary.delivered += 1,
                Err(error) => {
                    summary.dropped += 1;
                    warn!(subscriber_id, %error, "failed to forward media datagram");
                }
            }
        }
        summary
    }
}

fn media_kind_packet_type(media_kind: MediaKind) -> PacketType {
    match media_kind {
        MediaKind::Screen => PacketType::Video,
        MediaKind::Voice => PacketType::Audio,
        MediaKind::Probe => PacketType::Probe,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaForwardSummary {
    pub stream_id: u32,
    pub delivered: u32,
    pub dropped: u32,
}

pub async fn serve_media_datagrams(
    state: &Mutex<ControlState>,
    media: &Mutex<MediaRelay>,
    connection: Connection,
    user_id: UserId,
) -> anyhow::Result<()> {
    loop {
        let bytes = match connection.read_datagram().await {
            Ok(bytes) => bytes,
            Err(quinn::ConnectionError::ApplicationClosed(_))
            | Err(quinn::ConnectionError::LocallyClosed)
            | Err(quinn::ConnectionError::TimedOut) => break,
            Err(error) => return Err(error).context("failed to read media datagram"),
        };
        let packet = match MediaPacket::decode(&bytes) {
            Ok(packet) => packet,
            Err(error) => {
                debug!(user_id, %error, "dropping malformed media datagram");
                continue;
            }
        };
        let state = state.lock().await;
        let media = media.lock().await;
        let summary = media.forward_media_packet(&state, user_id, &packet);
        debug!(
            user_id,
            stream_id = summary.stream_id,
            delivered = summary.delivered,
            dropped = summary.dropped,
            "forwarded media datagram"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use teamview_protocol::{
        PROTOCOL_VERSION,
        codec::CodecId,
        control::{
            ClientControl, ClientEnvelope, CreateRoom, Hello, JoinRoom, MediaKind, PublishStream,
            SubscribeStream,
        },
        packet::{MediaPacketHeader, PacketFlags, PacketType},
    };

    use crate::session::Session;

    use super::*;

    #[test]
    fn rejects_media_from_non_publisher() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        let room_id = setup_published_stream(&mut state, &mut publisher, &mut viewer);
        let packet = synthetic_packet(9);
        assert_eq!(room_id, 1);

        let relay = MediaRelay::new();
        let summary = relay.forward_media_packet(&state, viewer.user_id.unwrap(), &packet);

        assert_eq!(summary.delivered, 0);
        assert_eq!(summary.dropped, 1);
    }

    #[test]
    fn rejects_media_with_mismatched_packet_type() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        setup_published_stream(&mut state, &mut publisher, &mut viewer);
        let mut packet = synthetic_packet(9);
        packet.header.packet_type = PacketType::Audio;

        let relay = MediaRelay::new();
        let summary = relay.forward_media_packet(&state, publisher.user_id.unwrap(), &packet);

        assert_eq!(summary.delivered, 0);
        assert_eq!(summary.dropped, 1);
    }

    fn setup_published_stream(
        state: &mut ControlState,
        publisher: &mut Session,
        viewer: &mut Session,
    ) -> u64 {
        state.handle_client_envelope(
            publisher,
            ClientEnvelope::new(
                1,
                ClientControl::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "publisher".to_owned(),
                }),
            ),
        );
        state.handle_client_envelope(
            viewer,
            ClientEnvelope::new(
                1,
                ClientControl::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "viewer".to_owned(),
                }),
            ),
        );
        let created = state.handle_client_envelope(
            publisher,
            ClientEnvelope::new(
                2,
                ClientControl::CreateRoom(CreateRoom {
                    name: "stage1".to_owned(),
                }),
            ),
        );
        let room_id = match created.message {
            teamview_protocol::control::ServerControl::RoomCreated(room) => room.room_id,
            other => panic!("unexpected create response: {other:?}"),
        };
        state.handle_client_envelope(
            publisher,
            ClientEnvelope::new(3, ClientControl::JoinRoom(JoinRoom { room_id })),
        );
        state.handle_client_envelope(
            viewer,
            ClientEnvelope::new(3, ClientControl::JoinRoom(JoinRoom { room_id })),
        );
        state.handle_client_envelope(
            publisher,
            ClientEnvelope::new(
                4,
                ClientControl::PublishStream(PublishStream {
                    room_id,
                    stream_id: 9,
                    codec: CodecId::H264,
                    media_kind: MediaKind::Screen,
                }),
            ),
        );
        state.handle_client_envelope(
            viewer,
            ClientEnvelope::new(
                5,
                ClientControl::SubscribeStream(SubscribeStream {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );
        room_id
    }

    fn synthetic_packet(stream_id: u32) -> MediaPacket {
        let payload = Bytes::from_static(b"synthetic");
        let mut header = MediaPacketHeader::new(
            PacketType::Video,
            CodecId::H264,
            stream_id,
            1,
            payload.len() as u16,
        );
        header.flags = PacketFlags::empty().with(PacketFlags::END_OF_FRAME);
        MediaPacket { header, payload }
    }
}
