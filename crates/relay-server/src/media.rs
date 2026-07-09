use std::collections::BTreeMap;

use anyhow::Context;
use bytes::Bytes;
use quinn::Connection;
use teamview_protocol::{
    control::{MediaKind, UserId},
    packet::{MediaPacket, PacketType},
};
use tokio::{
    sync::{
        Mutex,
        mpsc::{self, error::TrySendError},
    },
    task::JoinHandle,
};
use tracing::{debug, warn};

use crate::{
    control::ControlState,
    metrics::{micros_delta_to_millis, unix_time_micros},
};

const DEFAULT_EGRESS_QUEUE_CAPACITY: usize = 256;

#[derive(Debug)]
pub struct MediaRelay {
    egress: BTreeMap<UserId, ViewerEgress>,
    egress_queue_capacity: usize,
}

impl MediaRelay {
    pub fn new() -> Self {
        Self::with_egress_queue_capacity(DEFAULT_EGRESS_QUEUE_CAPACITY)
    }

    pub fn with_egress_queue_capacity(egress_queue_capacity: usize) -> Self {
        Self {
            egress: BTreeMap::new(),
            egress_queue_capacity: egress_queue_capacity.max(1),
        }
    }

    pub fn register(&mut self, user_id: UserId, connection: Connection) {
        self.unregister(user_id);
        let (sender, receiver) = mpsc::channel(self.egress_queue_capacity);
        let send_task = tokio::spawn(send_viewer_egress(user_id, connection, receiver));
        self.egress.insert(
            user_id,
            ViewerEgress {
                sender,
                send_task: Some(send_task),
            },
        );
    }

    pub fn unregister(&mut self, user_id: UserId) {
        self.egress.remove(&user_id);
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
                queued: 0,
                dropped: 1,
            };
        };
        if published.publisher_id != publisher_id
            || published.codec != packet.header.codec
            || media_kind_packet_type(published.media_kind) != packet.header.packet_type
        {
            return MediaForwardSummary {
                stream_id,
                queued: 0,
                dropped: 1,
            };
        }

        let Ok(encoded) = packet.encode() else {
            return MediaForwardSummary {
                stream_id,
                queued: 0,
                dropped: 1,
            };
        };

        let mut summary = MediaForwardSummary {
            stream_id,
            queued: 0,
            dropped: 0,
        };
        for subscriber_id in state.subscribers_for_stream(stream_id) {
            if subscriber_id == publisher_id {
                continue;
            }
            let Some(egress) = self.egress.get(&subscriber_id) else {
                summary.dropped += 1;
                continue;
            };
            match egress.sender.try_send(encoded.clone()) {
                Ok(()) => summary.queued += 1,
                Err(TrySendError::Full(_)) => {
                    summary.dropped += 1;
                    debug!(
                        subscriber_id,
                        "dropping media datagram for full viewer egress queue"
                    );
                }
                Err(TrySendError::Closed(_)) => {
                    summary.dropped += 1;
                    debug!(
                        subscriber_id,
                        "dropping media datagram for closed viewer egress queue"
                    );
                }
            }
        }
        summary
    }

    #[cfg(test)]
    fn register_paused_for_test(&mut self, user_id: UserId) -> mpsc::Receiver<Bytes> {
        self.unregister(user_id);
        let (sender, receiver) = mpsc::channel(self.egress_queue_capacity);
        self.egress.insert(
            user_id,
            ViewerEgress {
                sender,
                send_task: None,
            },
        );
        receiver
    }
}

impl Default for MediaRelay {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct ViewerEgress {
    sender: mpsc::Sender<Bytes>,
    send_task: Option<JoinHandle<()>>,
}

impl Drop for ViewerEgress {
    fn drop(&mut self) {
        if let Some(send_task) = self.send_task.take() {
            send_task.abort();
        }
    }
}

async fn send_viewer_egress(
    user_id: UserId,
    connection: Connection,
    mut receiver: mpsc::Receiver<Bytes>,
) {
    while let Some(bytes) = receiver.recv().await {
        if let Err(error) = connection.send_datagram(bytes) {
            warn!(user_id, %error, "failed to send queued media datagram");
        }
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
    pub queued: u32,
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
        let received_at_micros = unix_time_micros();
        let packet = match MediaPacket::decode(&bytes) {
            Ok(packet) => packet,
            Err(error) => {
                debug!(user_id, %error, "dropping malformed media datagram");
                continue;
            }
        };
        let mut state = state.lock().await;
        let media = media.lock().await;
        let summary = media.forward_media_packet(&state, user_id, &packet);
        let server_route_ms = micros_delta_to_millis(received_at_micros, unix_time_micros());
        state.record_media_forward_summary(
            &packet,
            summary,
            bytes.len(),
            received_at_micros,
            server_route_ms,
        );
        debug!(
            user_id,
            stream_id = summary.stream_id,
            queued = summary.queued,
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

        assert_eq!(summary.queued, 0);
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

        assert_eq!(summary.queued, 0);
        assert_eq!(summary.dropped, 1);
    }

    #[test]
    fn full_viewer_egress_queue_drops_only_for_that_viewer() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut fast_viewer = Session::anonymous(2);
        let mut slow_viewer = Session::anonymous(3);
        let room_id = setup_published_stream(&mut state, &mut publisher, &mut fast_viewer);
        join_and_subscribe(&mut state, &mut slow_viewer, room_id, 9);

        let mut relay = MediaRelay::with_egress_queue_capacity(1);
        let mut fast_rx = relay.register_paused_for_test(fast_viewer.user_id.unwrap());
        let mut slow_rx = relay.register_paused_for_test(slow_viewer.user_id.unwrap());

        let first = synthetic_packet_with_sequence(9, 1);
        let first_summary = relay.forward_media_packet(&state, publisher.user_id.unwrap(), &first);
        assert_eq!(first_summary.queued, 2);
        assert_eq!(first_summary.dropped, 0);

        let fast_first = fast_rx.try_recv().unwrap();
        assert_eq!(MediaPacket::decode(&fast_first).unwrap(), first);

        let second = synthetic_packet_with_sequence(9, 2);
        let second_summary =
            relay.forward_media_packet(&state, publisher.user_id.unwrap(), &second);
        assert_eq!(second_summary.queued, 1);
        assert_eq!(second_summary.dropped, 1);

        let fast_second = fast_rx.try_recv().unwrap();
        assert_eq!(MediaPacket::decode(&fast_second).unwrap(), second);
        let slow_first = slow_rx.try_recv().unwrap();
        assert_eq!(MediaPacket::decode(&slow_first).unwrap(), first);
        assert!(slow_rx.try_recv().is_err());
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

    fn join_and_subscribe(
        state: &mut ControlState,
        viewer: &mut Session,
        room_id: u64,
        stream_id: u32,
    ) {
        state.handle_client_envelope(
            viewer,
            ClientEnvelope::new(
                1,
                ClientControl::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: format!("viewer-{}", viewer.id),
                }),
            ),
        );
        state.handle_client_envelope(
            viewer,
            ClientEnvelope::new(2, ClientControl::JoinRoom(JoinRoom { room_id })),
        );
        state.handle_client_envelope(
            viewer,
            ClientEnvelope::new(
                3,
                ClientControl::SubscribeStream(SubscribeStream { room_id, stream_id }),
            ),
        );
    }

    fn synthetic_packet(stream_id: u32) -> MediaPacket {
        synthetic_packet_with_sequence(stream_id, 1)
    }

    fn synthetic_packet_with_sequence(stream_id: u32, sequence_number: u32) -> MediaPacket {
        let payload = Bytes::from_static(b"synthetic");
        let mut header = MediaPacketHeader::new(
            PacketType::Video,
            CodecId::H264,
            stream_id,
            sequence_number,
            payload.len() as u16,
        );
        header.frame_id = sequence_number;
        header.flags = PacketFlags::empty().with(PacketFlags::END_OF_FRAME);
        MediaPacket { header, payload }
    }
}
