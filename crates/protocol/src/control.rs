use serde::{Deserialize, Serialize};

use crate::codec::CodecId;

pub type RoomId = u64;
pub type UserId = u64;
pub type StreamId = u32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientControl {
    Hello(Hello),
    Authenticate(Authenticate),
    CreateRoom(CreateRoom),
    JoinRoom(JoinRoom),
    PublishStream(PublishStream),
    SubscribeStream(SubscribeStream),
    UnsubscribeStream(UnsubscribeStream),
    LeaveRoom(LeaveRoom),
    ViewerStats(ViewerStatsReport),
    SetTargetBitrate(SetTargetBitrate),
    SetTargetFramerate(SetTargetFramerate),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerControl {
    HelloAccepted(HelloAccepted),
    Authenticated(Authenticated),
    RoomCreated(RoomCreated),
    RoomJoined(RoomJoined),
    StreamPublished(StreamPublished),
    StreamSubscribed(StreamSubscribed),
    RequestKeyframe(RequestKeyframe),
    StreamConfig(StreamConfig),
    PublisherFeedback(PublisherFeedback),
    Error(ControlError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u8,
    pub client_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAccepted {
    pub protocol_version: u8,
    pub server_name: String,
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
pub struct JoinRoom {
    pub room_id: RoomId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomJoined {
    pub room_id: RoomId,
    pub participant_count: u32,
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
pub struct LeaveRoom {
    pub room_id: RoomId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestKeyframe {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub reason: KeyframeReason,
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
pub struct PublisherFeedback {
    pub room_id: RoomId,
    pub stream_id: StreamId,
    pub aggregate_available_bitrate_bps: u32,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PROTOCOL_VERSION;

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
}
