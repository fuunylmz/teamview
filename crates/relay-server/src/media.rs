use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicU32, Ordering},
    },
};

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
    room::PublishedStream,
};

const DEFAULT_EGRESS_QUEUE_CAPACITY: usize = 256;
const DEFAULT_VIEWER_QUEUE_BUDGET_MS: u16 = 100;

#[derive(Debug)]
pub struct MediaRelay {
    egress: BTreeMap<UserId, ViewerEgress>,
    egress_queue_capacity: usize,
    viewer_queue_budget_ms: u16,
}

impl MediaRelay {
    pub fn new() -> Self {
        Self::with_egress_limits(
            DEFAULT_EGRESS_QUEUE_CAPACITY,
            DEFAULT_VIEWER_QUEUE_BUDGET_MS,
        )
    }

    pub fn with_egress_queue_capacity(egress_queue_capacity: usize) -> Self {
        Self::with_egress_limits(egress_queue_capacity, DEFAULT_VIEWER_QUEUE_BUDGET_MS)
    }

    pub fn with_viewer_queue_budget_ms(viewer_queue_budget_ms: u16) -> Self {
        Self::with_egress_limits(DEFAULT_EGRESS_QUEUE_CAPACITY, viewer_queue_budget_ms)
    }

    pub fn with_egress_limits(egress_queue_capacity: usize, viewer_queue_budget_ms: u16) -> Self {
        Self {
            egress: BTreeMap::new(),
            egress_queue_capacity: egress_queue_capacity.max(1),
            viewer_queue_budget_ms: viewer_queue_budget_ms.max(1),
        }
    }

    pub fn register(&mut self, user_id: UserId, connection: Connection) {
        self.unregister(user_id);
        let (sender, receiver) = mpsc::channel(self.egress_queue_capacity);
        let queued_media_ms = Arc::new(AtomicU32::new(0));
        let stream_depths = Arc::new(StdMutex::new(BTreeMap::new()));
        let send_task = tokio::spawn(send_viewer_egress(
            user_id,
            connection,
            receiver,
            queued_media_ms.clone(),
            stream_depths.clone(),
        ));
        self.egress.insert(
            user_id,
            ViewerEgress {
                sender,
                queued_media_ms,
                stream_depths,
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
        let media_duration_ms =
            packet_media_duration_ms(packet, published, self.viewer_queue_budget_ms);

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
            if !egress.try_reserve_media(stream_id, media_duration_ms, self.viewer_queue_budget_ms)
            {
                summary.dropped += 1;
                debug!(
                    subscriber_id,
                    queued_media_ms = egress.queued_media_ms.load(Ordering::Relaxed),
                    budget_ms = self.viewer_queue_budget_ms,
                    "dropping media datagram for over-budget viewer egress queue"
                );
                continue;
            }
            let datagram = EgressDatagram {
                stream_id,
                bytes: encoded.clone(),
                media_duration_ms,
            };
            match egress.sender.try_send(datagram) {
                Ok(()) => summary.queued += 1,
                Err(TrySendError::Full(datagram)) => {
                    egress.release_media(datagram.stream_id, datagram.media_duration_ms);
                    summary.dropped += 1;
                    debug!(
                        subscriber_id,
                        "dropping media datagram for full viewer egress queue"
                    );
                }
                Err(TrySendError::Closed(datagram)) => {
                    egress.release_media(datagram.stream_id, datagram.media_duration_ms);
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
    fn register_paused_for_test(&mut self, user_id: UserId) -> mpsc::Receiver<EgressDatagram> {
        self.unregister(user_id);
        let (sender, receiver) = mpsc::channel(self.egress_queue_capacity);
        self.egress.insert(
            user_id,
            ViewerEgress {
                sender,
                queued_media_ms: Arc::new(AtomicU32::new(0)),
                stream_depths: Arc::new(StdMutex::new(BTreeMap::new())),
                send_task: None,
            },
        );
        receiver
    }

    pub fn stream_egress_depth(&self, state: &ControlState, stream_id: u32) -> EgressQueueDepth {
        state
            .subscribers_for_stream(stream_id)
            .into_iter()
            .filter_map(|subscriber_id| self.egress.get(&subscriber_id))
            .fold(EgressQueueDepth::default(), |mut total, egress| {
                let depth = egress.stream_depth(stream_id);
                total.queued_packets = total.queued_packets.saturating_add(depth.queued_packets);
                total.queued_media_ms = total.queued_media_ms.saturating_add(depth.queued_media_ms);
                total
            })
    }
}

impl Default for MediaRelay {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct ViewerEgress {
    sender: mpsc::Sender<EgressDatagram>,
    queued_media_ms: Arc<AtomicU32>,
    stream_depths: Arc<StdMutex<BTreeMap<u32, EgressQueueDepth>>>,
    send_task: Option<JoinHandle<()>>,
}

impl ViewerEgress {
    fn try_reserve_media(
        &self,
        stream_id: u32,
        media_duration_ms: u16,
        queue_budget_ms: u16,
    ) -> bool {
        let media_duration_ms = media_duration_ms as u32;
        let queue_budget_ms = queue_budget_ms as u32;
        let mut current = self.queued_media_ms.load(Ordering::Relaxed);
        loop {
            let Some(next) = current.checked_add(media_duration_ms) else {
                return false;
            };
            if next > queue_budget_ms {
                return false;
            }
            match self.queued_media_ms.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    reserve_stream_depth(&self.stream_depths, stream_id, media_duration_ms);
                    return true;
                }
                Err(actual) => current = actual,
            }
        }
    }

    fn release_media(&self, stream_id: u32, media_duration_ms: u16) {
        release_queued_media(&self.queued_media_ms, media_duration_ms);
        release_stream_depth(&self.stream_depths, stream_id, media_duration_ms);
    }

    fn stream_depth(&self, stream_id: u32) -> EgressQueueDepth {
        let Ok(depths) = self.stream_depths.lock() else {
            return EgressQueueDepth::default();
        };
        depths.get(&stream_id).copied().unwrap_or_default()
    }
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
    mut receiver: mpsc::Receiver<EgressDatagram>,
    queued_media_ms: Arc<AtomicU32>,
    stream_depths: Arc<StdMutex<BTreeMap<u32, EgressQueueDepth>>>,
) {
    while let Some(datagram) = receiver.recv().await {
        let stream_id = datagram.stream_id;
        let media_duration_ms = datagram.media_duration_ms;
        if let Err(error) = connection.send_datagram(datagram.bytes) {
            warn!(user_id, %error, "failed to send queued media datagram");
        }
        release_queued_media(&queued_media_ms, media_duration_ms);
        release_stream_depth(&stream_depths, stream_id, media_duration_ms);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EgressDatagram {
    stream_id: u32,
    bytes: Bytes,
    media_duration_ms: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EgressQueueDepth {
    pub queued_packets: u32,
    pub queued_media_ms: u16,
}

fn release_queued_media(queued_media_ms: &AtomicU32, media_duration_ms: u16) {
    let media_duration_ms = media_duration_ms as u32;
    if media_duration_ms == 0 {
        return;
    }
    let _ = queued_media_ms.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_sub(media_duration_ms))
    });
}

fn reserve_stream_depth(
    stream_depths: &StdMutex<BTreeMap<u32, EgressQueueDepth>>,
    stream_id: u32,
    media_duration_ms: u32,
) {
    let Ok(mut depths) = stream_depths.lock() else {
        return;
    };
    let depth = depths.entry(stream_id).or_default();
    depth.queued_packets = depth.queued_packets.saturating_add(1);
    depth.queued_media_ms = depth
        .queued_media_ms
        .saturating_add(media_duration_ms.min(u16::MAX as u32) as u16);
}

fn release_stream_depth(
    stream_depths: &StdMutex<BTreeMap<u32, EgressQueueDepth>>,
    stream_id: u32,
    media_duration_ms: u16,
) {
    let Ok(mut depths) = stream_depths.lock() else {
        return;
    };
    let Some(depth) = depths.get_mut(&stream_id) else {
        return;
    };
    depth.queued_packets = depth.queued_packets.saturating_sub(1);
    depth.queued_media_ms = depth.queued_media_ms.saturating_sub(media_duration_ms);
    if depth.queued_packets == 0 && depth.queued_media_ms == 0 {
        depths.remove(&stream_id);
    }
}

fn packet_media_duration_ms(
    packet: &MediaPacket,
    published: &PublishedStream,
    queue_budget_ms: u16,
) -> u16 {
    let fps = published
        .config
        .as_ref()
        .map(|config| config.frames_per_second)
        .unwrap_or(published.target_frames_per_second)
        .max(1) as u32;
    let frame_duration_ms = 1_000_u32.div_ceil(fps).min(queue_budget_ms.max(1) as u32);
    let fragment_count = packet.header.fragment_count.max(1) as u32;
    frame_duration_ms
        .saturating_div(fragment_count)
        .max(1)
        .min(u16::MAX as u32) as u16
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
        assert_eq!(MediaPacket::decode(&fast_first.bytes).unwrap(), first);

        let second = synthetic_packet_with_sequence(9, 2);
        let second_summary =
            relay.forward_media_packet(&state, publisher.user_id.unwrap(), &second);
        assert_eq!(second_summary.queued, 1);
        assert_eq!(second_summary.dropped, 1);

        let fast_second = fast_rx.try_recv().unwrap();
        assert_eq!(MediaPacket::decode(&fast_second.bytes).unwrap(), second);
        let slow_first = slow_rx.try_recv().unwrap();
        assert_eq!(MediaPacket::decode(&slow_first.bytes).unwrap(), first);
        assert!(slow_rx.try_recv().is_err());
    }

    #[test]
    fn viewer_egress_media_budget_drops_before_channel_capacity() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        setup_published_stream(&mut state, &mut publisher, &mut viewer);

        let mut relay = MediaRelay::with_egress_limits(8, 50);
        let mut viewer_rx = relay.register_paused_for_test(viewer.user_id.unwrap());

        let first = synthetic_packet_with_sequence(9, 1);
        let first_summary = relay.forward_media_packet(&state, publisher.user_id.unwrap(), &first);
        assert_eq!(first_summary.queued, 1);
        assert_eq!(first_summary.dropped, 0);
        assert_eq!(
            relay.stream_egress_depth(&state, 9),
            EgressQueueDepth {
                queued_packets: 1,
                queued_media_ms: 34,
            }
        );

        let second = synthetic_packet_with_sequence(9, 2);
        let second_summary =
            relay.forward_media_packet(&state, publisher.user_id.unwrap(), &second);
        assert_eq!(second_summary.queued, 0);
        assert_eq!(second_summary.dropped, 1);
        assert_eq!(
            relay.stream_egress_depth(&state, 9),
            EgressQueueDepth {
                queued_packets: 1,
                queued_media_ms: 34,
            }
        );

        let queued_first = viewer_rx.try_recv().unwrap();
        assert_eq!(MediaPacket::decode(&queued_first.bytes).unwrap(), first);
        assert!(viewer_rx.try_recv().is_err());
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
