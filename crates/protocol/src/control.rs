use serde::{Deserialize, Serialize};

use crate::{PROTOCOL_VERSION, codec::CodecId};

pub type RoomId = u64;
pub type UserId = u64;
pub type StreamId = u32;
pub type RequestId = u64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlEnvelope<T> {
    pub protocol_version: u8,
    pub request_id: RequestId,
    pub message: T,
}

impl<T> ControlEnvelope<T> {
    pub fn new(request_id: RequestId, message: T) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientControl {
    Hello(Hello),
    Ping(Ping),
    TimeSync(TimeSyncRequest),
    Authenticate(Authenticate),
    CreateRoom(CreateRoom),
    ListRooms(ListRooms),
    JoinRoom(JoinRoom),
    ListParticipants(ListParticipants),
    PublishStream(PublishStream),
    UnpublishStream(UnpublishStream),
    ListStreams(ListStreams),
    SubscribeStream(SubscribeStream),
    UnsubscribeStream(UnsubscribeStream),
    LeaveRoom(LeaveRoom),
    SetVoiceState(SetVoiceState),
    SetStreamConfig(StreamConfig),
    PollStreamConfig(PollStreamConfig),
    PollStreamMetrics(PollStreamMetrics),
    RequestKeyframe(RequestKeyframe),
    SendRemoteInput(SendRemoteInput),
    PollRemoteInput(PollRemoteInput),
    ViewerStats(ViewerStatsReport),
    PollPublisherFeedback(PollPublisherFeedback),
    SetTargetBitrate(SetTargetBitrate),
    SetTargetFramerate(SetTargetFramerate),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerControl {
    HelloAccepted(HelloAccepted),
    Pong(Pong),
    TimeSync(TimeSyncResponse),
    Authenticated(Authenticated),
    RoomCreated(RoomCreated),
    RoomList(RoomList),
    RoomJoined(RoomJoined),
    ParticipantList(ParticipantList),
    StreamPublished(StreamPublished),
    StreamUnpublished(StreamUnpublished),
    StreamList(StreamList),
    StreamSubscribed(StreamSubscribed),
    StreamUnsubscribed(StreamUnsubscribed),
    RoomLeft(RoomLeft),
    VoiceStateUpdated(VoiceState),
    RequestKeyframe(RequestKeyframe),
    StreamConfig(StreamConfig),
    StreamMetrics(StreamMetricsSnapshot),
    RemoteInputQueued(RemoteInputQueued),
    RemoteInputBatch(RemoteInputBatch),
    PublisherFeedback(PublisherFeedback),
    Error(ControlError),
}

pub type ClientEnvelope = ControlEnvelope<ClientControl>;
pub type ServerEnvelope = ControlEnvelope<ServerControl>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u8,
    pub client_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAccepted {
    pub protocol_version: u8,
    pub server_name: String,
    pub user_id: UserId,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ping {
    pub nonce: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pong {
    pub nonce: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSyncRequest {
    pub client_send_time_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSyncResponse {
    pub client_send_time_micros: u64,
    pub server_receive_time_micros: u64,
    pub server_send_time_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authenticate {
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authenticated {
    pub user_id: UserId,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRoom {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomCreated {
    pub room_id: RoomId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRooms;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSummary {
    pub room_id: RoomId,
    pub name: String,
    pub participant_count: u32,
    pub published_stream_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomList {
    pub rooms: Vec<RoomSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinRoom {
    pub room_id: RoomId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomJoined {
    pub room_id: RoomId,
    pub participant_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListParticipants {
    pub room_id: RoomId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantSummary {
    pub room_id: RoomId,
    pub user_id: UserId,
    pub display_name: String,
    pub muted: bool,
    pub deafened: bool,
    pub push_to_talk: bool,
    pub speaking: bool,
    pub published_stream_count: u32,
    pub subscribed_stream_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantList {
    pub room_id: RoomId,
    pub participants: Vec<ParticipantSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishStream {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub codec: CodecId,
    pub media_kind: MediaKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamPublished {
    pub room_id: RoomId,
    pub stream_id: StreamId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnpublishStream {
    pub room_id: RoomId,
    pub stream_id: StreamId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamUnpublished {
    pub room_id: RoomId,
    pub stream_id: StreamId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListStreams {
    pub room_id: RoomId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamSummary {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub publisher_id: UserId,
    pub codec: CodecId,
    pub media_kind: MediaKind,
    pub subscriber_count: u32,
    pub has_config: bool,
    pub target_bitrate_bps: u32,
    pub target_frames_per_second: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamList {
    pub room_id: RoomId,
    pub streams: Vec<StreamSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeStream {
    pub room_id: RoomId,
    pub stream_id: StreamId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamSubscribed {
    pub room_id: RoomId,
    pub stream_id: StreamId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsubscribeStream {
    pub room_id: RoomId,
    pub stream_id: StreamId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamUnsubscribed {
    pub room_id: RoomId,
    pub stream_id: StreamId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaveRoom {
    pub room_id: RoomId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomLeft {
    pub room_id: RoomId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetVoiceState {
    pub room_id: RoomId,
    pub muted: bool,
    pub deafened: bool,
    pub push_to_talk: bool,
    pub speaking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceState {
    pub room_id: RoomId,
    pub user_id: UserId,
    pub muted: bool,
    pub deafened: bool,
    pub push_to_talk: bool,
    pub speaking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestKeyframe {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub reason: KeyframeReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendRemoteInput {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub sequence_number: u64,
    pub event_time_micros: u64,
    pub kind: RemoteInputKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollRemoteInput {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub max_events: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteInputQueued {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub publisher_id: UserId,
    pub queued_events: u16,
    pub dropped_events: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteInputBatch {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub events: Vec<RemoteInputEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteInputEvent {
    pub sender_user_id: UserId,
    pub sequence_number: u64,
    pub event_time_micros: u64,
    pub kind: RemoteInputKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteInputKind {
    PointerMove {
        normalized_x: u16,
        normalized_y: u16,
    },
    PointerButton {
        button: PointerButton,
        pressed: bool,
        normalized_x: u16,
        normalized_y: u16,
    },
    PointerWheel {
        delta_x: i16,
        delta_y: i16,
        normalized_x: u16,
        normalized_y: u16,
    },
    Key {
        key_code: u16,
        pressed: bool,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamConfig {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub codec: CodecId,
    pub width: u32,
    pub height: u32,
    pub frames_per_second: u16,
    pub timebase_hz: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollStreamConfig {
    pub room_id: RoomId,
    pub stream_id: StreamId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollStreamMetrics {
    pub room_id: RoomId,
    pub stream_id: StreamId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamMetricsSnapshot {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub ingress_packets: u64,
    pub ingress_bytes: u64,
    pub egress_queued_packets: u64,
    pub egress_dropped_packets: u64,
    pub egress_queue_packets: u32,
    pub egress_queue_media_ms: u16,
    pub subscriber_count: u32,
    pub last_ingress_time_micros: u64,
    pub server_route_ms_p50: u16,
    pub server_route_ms_p95: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublisherFeedback {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub aggregate_available_bitrate_bps: u32,
    pub target_frames_per_second: u16,
    pub target_width: u32,
    pub target_height: u32,
    pub degraded_viewer_count: u32,
    pub total_viewer_count: u32,
    pub keyframe_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerStatsReport {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub received_packets: u64,
    pub lost_packets: u64,
    pub decoded_frames: u64,
    pub dropped_frames: u64,
    pub jitter_buffer_ms: u16,
    pub estimated_latency_ms: u16,
    pub calibrated_latency_ms: u16,
    pub reassembly_ms_p50: u16,
    pub reassembly_ms_p95: u16,
    pub decode_ms_p50: u16,
    pub decode_ms_p95: u16,
    pub render_ms_p50: u16,
    pub render_ms_p95: u16,
    pub render_fps: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollPublisherFeedback {
    pub room_id: RoomId,
    pub stream_id: StreamId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetTargetBitrate {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub bitrate_bps: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetTargetFramerate {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub frames_per_second: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Screen,
    Voice,
    Probe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyframeReason {
    NewSubscriber,
    PacketLoss,
    DecoderRecovery,
    StreamConfigChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlError {
    pub code: String,
    pub message: String,
}

impl ControlError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ControlCodecError {
    #[error("control message json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported control protocol version {0}")]
    UnsupportedVersion(u8),
}

pub fn encode_client_envelope(envelope: &ClientEnvelope) -> Result<Vec<u8>, ControlCodecError> {
    encode_envelope(envelope)
}

pub fn decode_client_envelope(bytes: &[u8]) -> Result<ClientEnvelope, ControlCodecError> {
    decode_envelope(bytes)
}

pub fn encode_server_envelope(envelope: &ServerEnvelope) -> Result<Vec<u8>, ControlCodecError> {
    encode_envelope(envelope)
}

pub fn decode_server_envelope(bytes: &[u8]) -> Result<ServerEnvelope, ControlCodecError> {
    decode_envelope(bytes)
}

fn encode_envelope<T: Serialize>(
    envelope: &ControlEnvelope<T>,
) -> Result<Vec<u8>, ControlCodecError> {
    let mut bytes = serde_json::to_vec(envelope)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn decode_envelope<T>(bytes: &[u8]) -> Result<ControlEnvelope<T>, ControlCodecError>
where
    T: for<'de> Deserialize<'de>,
{
    let envelope: ControlEnvelope<T> = serde_json::from_slice(bytes.trim_ascii())?;
    if envelope.protocol_version != PROTOCOL_VERSION {
        return Err(ControlCodecError::UnsupportedVersion(
            envelope.protocol_version,
        ));
    }
    Ok(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_carries_protocol_version() {
        let hello = ClientControl::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "desktop-client".to_owned(),
        });

        assert!(matches!(
            hello,
            ClientControl::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                ..
            })
        ));
    }

    #[test]
    fn publish_stream_identifies_media_kind_and_codec() {
        let publish = PublishStream {
            room_id: 1,
            stream_id: 9,
            codec: CodecId::H264,
            media_kind: MediaKind::Screen,
        };

        assert_eq!(publish.codec, CodecId::H264);
        assert_eq!(publish.media_kind, MediaKind::Screen);
    }

    #[test]
    fn unpublish_stream_round_trips_as_json_line() {
        let request = ClientEnvelope::new(
            9,
            ClientControl::UnpublishStream(UnpublishStream {
                room_id: 1,
                stream_id: 9,
            }),
        );
        let response = ServerEnvelope::new(
            9,
            ServerControl::StreamUnpublished(StreamUnpublished {
                room_id: 1,
                stream_id: 9,
            }),
        );

        let encoded_request = encode_client_envelope(&request).unwrap();
        let encoded_response = encode_server_envelope(&response).unwrap();

        assert_eq!(decode_client_envelope(&encoded_request).unwrap(), request);
        assert_eq!(decode_server_envelope(&encoded_response).unwrap(), response);
    }

    #[test]
    fn client_envelope_round_trips_as_json_line() {
        let envelope = ClientEnvelope::new(
            7,
            ClientControl::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                client_name: "desktop-client".to_owned(),
            }),
        );

        let encoded = encode_client_envelope(&envelope).unwrap();
        assert_eq!(encoded.last(), Some(&b'\n'));
        assert_eq!(decode_client_envelope(&encoded).unwrap(), envelope);
    }

    #[test]
    fn ping_round_trips_as_json_line() {
        let envelope = ClientEnvelope::new(8, ClientControl::Ping(Ping { nonce: 42 }));

        let encoded = encode_client_envelope(&envelope).unwrap();

        assert_eq!(decode_client_envelope(&encoded).unwrap(), envelope);
    }

    #[test]
    fn time_sync_round_trips_as_json_lines() {
        let request = ClientEnvelope::new(
            8,
            ClientControl::TimeSync(TimeSyncRequest {
                client_send_time_micros: 1_000_000,
            }),
        );
        let response = ServerEnvelope::new(
            8,
            ServerControl::TimeSync(TimeSyncResponse {
                client_send_time_micros: 1_000_000,
                server_receive_time_micros: 1_000_500,
                server_send_time_micros: 1_000_700,
            }),
        );

        let encoded_request = encode_client_envelope(&request).unwrap();
        let encoded_response = encode_server_envelope(&response).unwrap();

        assert_eq!(decode_client_envelope(&encoded_request).unwrap(), request);
        assert_eq!(decode_server_envelope(&encoded_response).unwrap(), response);
    }

    #[test]
    fn publisher_feedback_round_trips_from_server() {
        let envelope = ServerEnvelope::new(
            8,
            ServerControl::PublisherFeedback(PublisherFeedback {
                room_id: 1,
                stream_id: 9,
                aggregate_available_bitrate_bps: 1_200_000,
                target_frames_per_second: 24,
                target_width: 1280,
                target_height: 720,
                degraded_viewer_count: 1,
                total_viewer_count: 3,
                keyframe_requested: true,
            }),
        );

        let encoded = encode_server_envelope(&envelope).unwrap();

        assert_eq!(decode_server_envelope(&encoded).unwrap(), envelope);
    }

    #[test]
    fn keyframe_request_round_trips_from_client() {
        let envelope = ClientEnvelope::new(
            8,
            ClientControl::RequestKeyframe(RequestKeyframe {
                room_id: 1,
                stream_id: 9,
                reason: KeyframeReason::DecoderRecovery,
            }),
        );

        let encoded = encode_client_envelope(&envelope).unwrap();

        assert_eq!(decode_client_envelope(&encoded).unwrap(), envelope);
    }

    #[test]
    fn remote_input_round_trips_as_control_messages() {
        let request = ClientEnvelope::new(
            8,
            ClientControl::SendRemoteInput(SendRemoteInput {
                room_id: 1,
                stream_id: 9,
                sequence_number: 3,
                event_time_micros: 1_234,
                kind: RemoteInputKind::PointerButton {
                    button: PointerButton::Left,
                    pressed: true,
                    normalized_x: 32_768,
                    normalized_y: 16_384,
                },
            }),
        );
        let queued = ServerEnvelope::new(
            8,
            ServerControl::RemoteInputQueued(RemoteInputQueued {
                room_id: 1,
                stream_id: 9,
                publisher_id: 2,
                queued_events: 1,
                dropped_events: 0,
            }),
        );
        let poll = ClientEnvelope::new(
            9,
            ClientControl::PollRemoteInput(PollRemoteInput {
                room_id: 1,
                stream_id: 9,
                max_events: 16,
            }),
        );
        let batch = ServerEnvelope::new(
            9,
            ServerControl::RemoteInputBatch(RemoteInputBatch {
                room_id: 1,
                stream_id: 9,
                events: vec![RemoteInputEvent {
                    sender_user_id: 7,
                    sequence_number: 3,
                    event_time_micros: 1_234,
                    kind: RemoteInputKind::Key {
                        key_code: 13,
                        pressed: false,
                    },
                }],
            }),
        );

        assert_eq!(
            decode_client_envelope(&encode_client_envelope(&request).unwrap()).unwrap(),
            request
        );
        assert_eq!(
            decode_server_envelope(&encode_server_envelope(&queued).unwrap()).unwrap(),
            queued
        );
        assert_eq!(
            decode_client_envelope(&encode_client_envelope(&poll).unwrap()).unwrap(),
            poll
        );
        assert_eq!(
            decode_server_envelope(&encode_server_envelope(&batch).unwrap()).unwrap(),
            batch
        );
    }

    #[test]
    fn viewer_stats_round_trips_from_client() {
        let envelope = ClientEnvelope::new(
            9,
            ClientControl::ViewerStats(ViewerStatsReport {
                room_id: 1,
                stream_id: 9,
                received_packets: 20,
                lost_packets: 1,
                decoded_frames: 8,
                dropped_frames: 2,
                jitter_buffer_ms: 33,
                estimated_latency_ms: 88,
                calibrated_latency_ms: 86,
                reassembly_ms_p50: 4,
                reassembly_ms_p95: 9,
                decode_ms_p50: 6,
                decode_ms_p95: 12,
                render_ms_p50: 3,
                render_ms_p95: 7,
                render_fps: 30,
            }),
        );

        let encoded = encode_client_envelope(&envelope).unwrap();

        assert_eq!(decode_client_envelope(&encoded).unwrap(), envelope);
    }

    #[test]
    fn voice_state_round_trips_as_control_messages() {
        let request = ClientEnvelope::new(
            9,
            ClientControl::SetVoiceState(SetVoiceState {
                room_id: 1,
                muted: true,
                deafened: false,
                push_to_talk: true,
                speaking: false,
            }),
        );
        let response = ServerEnvelope::new(
            9,
            ServerControl::VoiceStateUpdated(VoiceState {
                room_id: 1,
                user_id: 7,
                muted: true,
                deafened: false,
                push_to_talk: true,
                speaking: false,
            }),
        );

        let encoded_request = encode_client_envelope(&request).unwrap();
        let encoded_response = encode_server_envelope(&response).unwrap();

        assert_eq!(decode_client_envelope(&encoded_request).unwrap(), request);
        assert_eq!(decode_server_envelope(&encoded_response).unwrap(), response);
    }

    #[test]
    fn discovery_controls_round_trip_as_json_lines() {
        let list_rooms = ClientEnvelope::new(9, ClientControl::ListRooms(ListRooms));
        let list_streams =
            ClientEnvelope::new(10, ClientControl::ListStreams(ListStreams { room_id: 1 }));
        let list_participants = ClientEnvelope::new(
            11,
            ClientControl::ListParticipants(ListParticipants { room_id: 1 }),
        );
        let room_list = ServerEnvelope::new(
            11,
            ServerControl::RoomList(RoomList {
                rooms: vec![RoomSummary {
                    room_id: 1,
                    name: "stage1".to_owned(),
                    participant_count: 2,
                    published_stream_count: 1,
                }],
            }),
        );
        let stream_list = ServerEnvelope::new(
            12,
            ServerControl::StreamList(StreamList {
                room_id: 1,
                streams: vec![StreamSummary {
                    room_id: 1,
                    stream_id: 9,
                    publisher_id: 1,
                    codec: CodecId::H264,
                    media_kind: MediaKind::Screen,
                    subscriber_count: 1,
                    has_config: true,
                    target_bitrate_bps: 4_000_000,
                    target_frames_per_second: 30,
                }],
            }),
        );
        let participant_list = ServerEnvelope::new(
            13,
            ServerControl::ParticipantList(ParticipantList {
                room_id: 1,
                participants: vec![ParticipantSummary {
                    room_id: 1,
                    user_id: 7,
                    display_name: "user-7".to_owned(),
                    muted: false,
                    deafened: false,
                    push_to_talk: true,
                    speaking: true,
                    published_stream_count: 1,
                    subscribed_stream_count: 2,
                }],
            }),
        );

        let encoded_rooms = encode_client_envelope(&list_rooms).unwrap();
        let encoded_streams = encode_client_envelope(&list_streams).unwrap();
        let encoded_participants = encode_client_envelope(&list_participants).unwrap();
        let encoded_room_list = encode_server_envelope(&room_list).unwrap();
        let encoded_stream_list = encode_server_envelope(&stream_list).unwrap();
        let encoded_participant_list = encode_server_envelope(&participant_list).unwrap();

        assert_eq!(decode_client_envelope(&encoded_rooms).unwrap(), list_rooms);
        assert_eq!(
            decode_client_envelope(&encoded_streams).unwrap(),
            list_streams
        );
        assert_eq!(
            decode_client_envelope(&encoded_participants).unwrap(),
            list_participants
        );
        assert_eq!(
            decode_server_envelope(&encoded_room_list).unwrap(),
            room_list
        );
        assert_eq!(
            decode_server_envelope(&encoded_stream_list).unwrap(),
            stream_list
        );
        assert_eq!(
            decode_server_envelope(&encoded_participant_list).unwrap(),
            participant_list
        );
    }

    #[test]
    fn stream_metrics_round_trips_from_server() {
        let envelope = ServerEnvelope::new(
            9,
            ServerControl::StreamMetrics(StreamMetricsSnapshot {
                room_id: 1,
                stream_id: 9,
                ingress_packets: 3,
                ingress_bytes: 900,
                egress_queued_packets: 6,
                egress_dropped_packets: 1,
                egress_queue_packets: 2,
                egress_queue_media_ms: 34,
                subscriber_count: 2,
                last_ingress_time_micros: 1_700_000,
                server_route_ms_p50: 2,
                server_route_ms_p95: 3,
            }),
        );

        let encoded = encode_server_envelope(&envelope).unwrap();

        assert_eq!(decode_server_envelope(&encoded).unwrap(), envelope);
    }

    #[test]
    fn rejects_unsupported_envelope_version() {
        let encoded = br#"{"protocol_version":99,"request_id":1,"message":{"Hello":{"protocol_version":1,"client_name":"bad"}}}"#;

        assert!(matches!(
            decode_client_envelope(encoded),
            Err(ControlCodecError::UnsupportedVersion(99))
        ));
    }
}
