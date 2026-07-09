use serde::{Deserialize, Serialize};
use teamview_protocol::{
    codec::CodecId,
    control::{
        MediaKind, ParticipantSummary, RoomId as ChannelId, RoomSummary, StreamConfig, StreamId,
        StreamSummary, UserId,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientRole {
    Broadcaster,
    Viewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientApp {
    pub role: ClientRole,
    pub connection: ConnectionStatus,
    pub relay_addr: String,
    pub selected_channel_id: Option<ChannelId>,
    pub channels: Vec<ChannelView>,
    pub local_voice: VoiceControlState,
    pub local_screen_share: ScreenShareControlState,
    pub status_line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelView {
    pub channel_id: ChannelId,
    pub name: String,
    pub participant_count: u32,
    pub published_stream_count: u32,
    pub participants: Vec<ParticipantView>,
    pub screen_stream: Option<ScreenStreamView>,
    pub voice_stream: Option<VoiceStreamView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantView {
    pub user_id: UserId,
    pub display_name: String,
    pub muted: bool,
    pub deafened: bool,
    pub push_to_talk: bool,
    pub speaking: bool,
    pub sharing_screen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenStreamView {
    pub stream_id: StreamId,
    pub publisher_id: UserId,
    pub codec: CodecId,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub frames_per_second: u16,
    pub subscribed: bool,
    pub rendered_frames: u64,
    pub dropped_frames: u64,
    pub latency_ms: u16,
    pub bitrate_bps: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStreamView {
    pub stream_id: StreamId,
    pub publisher_id: UserId,
    pub codec: CodecId,
    pub frames_per_second: u16,
    pub subscribed: bool,
    pub decoded_frames: u64,
    pub dropped_frames: u64,
    pub latency_ms: u16,
    pub bitrate_bps: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceControlState {
    pub muted: bool,
    pub deafened: bool,
    pub push_to_talk: bool,
    pub ptt_active: bool,
    pub input_label: String,
    pub output_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenShareControlState {
    pub sharing: bool,
    pub stream_id: StreamId,
    pub source_label: String,
    pub target_width: u32,
    pub target_height: u32,
    pub target_fps: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct ClientAppDiscovery<'a> {
    pub role: ClientRole,
    pub relay_addr: &'a str,
    pub selected_channel_id: Option<ChannelId>,
    pub rooms: &'a [RoomSummary],
    pub streams: &'a [StreamSummary],
    pub stream_configs: &'a [StreamConfig],
    pub participants: &'a [ParticipantSummary],
    pub local_voice: &'a VoiceControlState,
    pub local_screen_share: &'a ScreenShareControlState,
}

impl ClientApp {
    pub fn new(role: ClientRole) -> Self {
        Self {
            role,
            connection: ConnectionStatus::Disconnected,
            relay_addr: "127.0.0.1:4433".to_owned(),
            selected_channel_id: None,
            channels: Vec::new(),
            local_voice: VoiceControlState::default(),
            local_screen_share: ScreenShareControlState::default(),
            status_line: "Disconnected".to_owned(),
        }
    }

    pub fn from_discovery(discovery: ClientAppDiscovery<'_>) -> Self {
        let relay_addr = discovery.relay_addr.to_owned();
        let mut app = Self::new(discovery.role);
        app.connection = ConnectionStatus::Connected;
        app.relay_addr = relay_addr.clone();
        app.status_line = format!("Connected to {relay_addr}");
        app.local_voice = discovery.local_voice.clone();
        app.local_screen_share = discovery.local_screen_share.clone();

        let selected_channel_id = discovery
            .selected_channel_id
            .or_else(|| discovery.rooms.first().map(|room| room.room_id));
        let channels = discovery
            .rooms
            .iter()
            .map(|room| {
                let is_selected = Some(room.room_id) == selected_channel_id;
                ChannelView::from_discovery(
                    room,
                    if is_selected { discovery.streams } else { &[] },
                    if is_selected {
                        discovery.stream_configs
                    } else {
                        &[]
                    },
                    if is_selected {
                        discovery.participants
                    } else {
                        &[]
                    },
                )
            })
            .collect();
        app.set_channels(channels);
        if let Some(channel_id) = selected_channel_id {
            app.select_channel(channel_id);
        }
        app
    }

    pub fn connecting(&mut self, relay_addr: impl Into<String>) {
        self.connection = ConnectionStatus::Connecting;
        self.relay_addr = relay_addr.into();
        self.status_line = format!("Connecting to {}", self.relay_addr);
    }

    pub fn connected(&mut self) {
        self.connection = ConnectionStatus::Connected;
        self.status_line = format!("Connected to {}", self.relay_addr);
    }

    pub fn disconnected(&mut self) {
        self.connection = ConnectionStatus::Disconnected;
        self.selected_channel_id = None;
        self.status_line = "Disconnected".to_owned();
    }

    pub fn set_channels(&mut self, channels: Vec<ChannelView>) {
        self.channels = channels;
        if let Some(selected_channel_id) = self.selected_channel_id
            && !self
                .channels
                .iter()
                .any(|channel| channel.channel_id == selected_channel_id)
        {
            self.selected_channel_id = None;
        }
        if self.selected_channel_id.is_none() {
            self.selected_channel_id = self.channels.first().map(|channel| channel.channel_id);
        }
    }

    pub fn select_channel(&mut self, channel_id: ChannelId) -> bool {
        if self
            .channels
            .iter()
            .any(|channel| channel.channel_id == channel_id)
        {
            self.selected_channel_id = Some(channel_id);
            true
        } else {
            false
        }
    }

    pub fn selected_channel(&self) -> Option<&ChannelView> {
        let channel_id = self.selected_channel_id?;
        self.channels
            .iter()
            .find(|channel| channel.channel_id == channel_id)
    }

    pub fn selected_channel_mut(&mut self) -> Option<&mut ChannelView> {
        let channel_id = self.selected_channel_id?;
        self.channels
            .iter_mut()
            .find(|channel| channel.channel_id == channel_id)
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.local_voice.muted = muted;
        if muted {
            self.local_voice.ptt_active = false;
        }
    }

    pub fn set_deafened(&mut self, deafened: bool) {
        self.local_voice.deafened = deafened;
        if deafened {
            self.set_muted(true);
        }
    }

    pub fn set_push_to_talk(&mut self, enabled: bool) {
        self.local_voice.push_to_talk = enabled;
        if !enabled {
            self.local_voice.ptt_active = false;
        }
    }

    pub fn set_ptt_active(&mut self, active: bool) {
        self.local_voice.ptt_active =
            active && self.local_voice.push_to_talk && !self.local_voice.muted;
    }

    pub fn local_speaking(&self) -> bool {
        !self.local_voice.muted && (!self.local_voice.push_to_talk || self.local_voice.ptt_active)
    }

    pub fn start_screen_share(&mut self, source_label: impl Into<String>) {
        self.local_screen_share.sharing = true;
        self.local_screen_share.source_label = source_label.into();
    }

    pub fn stop_screen_share(&mut self) {
        self.local_screen_share.sharing = false;
    }
}

impl ChannelView {
    pub fn new(channel_id: ChannelId, name: impl Into<String>) -> Self {
        Self {
            channel_id,
            name: name.into(),
            participant_count: 0,
            published_stream_count: 0,
            participants: Vec::new(),
            screen_stream: None,
            voice_stream: None,
        }
    }

    pub fn from_discovery(
        room: &RoomSummary,
        streams: &[StreamSummary],
        stream_configs: &[StreamConfig],
        participants: &[ParticipantSummary],
    ) -> Self {
        let mut channel = Self {
            channel_id: room.room_id,
            name: room.name.clone(),
            participant_count: if participants.is_empty() {
                room.participant_count
            } else {
                participants.len().min(u32::MAX as usize) as u32
            },
            published_stream_count: room.published_stream_count,
            participants: participants
                .iter()
                .map(|participant| ParticipantView::from_summary(participant, streams))
                .collect(),
            screen_stream: None,
            voice_stream: None,
        };

        channel.screen_stream = streams
            .iter()
            .filter(|stream| {
                stream.room_id == room.room_id && stream.media_kind == MediaKind::Screen
            })
            .min_by_key(|stream| stream.stream_id)
            .map(|stream| ScreenStreamView::from_summary(stream, stream_configs));
        channel.voice_stream = streams
            .iter()
            .filter(|stream| {
                stream.room_id == room.room_id && stream.media_kind == MediaKind::Voice
            })
            .min_by_key(|stream| stream.stream_id)
            .map(|stream| VoiceStreamView::from_summary(stream, stream_configs));
        channel
    }

    pub fn active_speaker_count(&self) -> usize {
        self.participants
            .iter()
            .filter(|participant| participant.speaking && !participant.muted)
            .count()
    }

    pub fn screen_share_active(&self) -> bool {
        self.screen_stream.is_some()
            || self
                .participants
                .iter()
                .any(|participant| participant.sharing_screen)
    }
}

impl ParticipantView {
    pub fn from_summary(participant: &ParticipantSummary, streams: &[StreamSummary]) -> Self {
        Self {
            user_id: participant.user_id,
            display_name: participant.display_name.clone(),
            muted: participant.muted,
            deafened: participant.deafened,
            push_to_talk: participant.push_to_talk,
            speaking: participant.speaking,
            sharing_screen: streams.iter().any(|stream| {
                stream.publisher_id == participant.user_id && stream.media_kind == MediaKind::Screen
            }),
        }
    }
}

impl ScreenStreamView {
    pub fn from_summary(stream: &StreamSummary, stream_configs: &[StreamConfig]) -> Self {
        let config = stream_config_for(stream, stream_configs);
        Self {
            stream_id: stream.stream_id,
            publisher_id: stream.publisher_id,
            codec: stream.codec,
            title: format!("Screen stream {}", stream.stream_id),
            width: config.map(|config| config.width).unwrap_or_default(),
            height: config.map(|config| config.height).unwrap_or_default(),
            frames_per_second: config
                .map(|config| config.frames_per_second)
                .unwrap_or(stream.target_frames_per_second),
            subscribed: false,
            rendered_frames: 0,
            dropped_frames: 0,
            latency_ms: 0,
            bitrate_bps: stream.target_bitrate_bps,
        }
    }
}

impl VoiceStreamView {
    pub fn from_summary(stream: &StreamSummary, stream_configs: &[StreamConfig]) -> Self {
        let config = stream_config_for(stream, stream_configs);
        Self {
            stream_id: stream.stream_id,
            publisher_id: stream.publisher_id,
            codec: stream.codec,
            frames_per_second: config
                .map(|config| config.frames_per_second)
                .unwrap_or(stream.target_frames_per_second),
            subscribed: false,
            decoded_frames: 0,
            dropped_frames: 0,
            latency_ms: 0,
            bitrate_bps: stream.target_bitrate_bps,
        }
    }
}

fn stream_config_for<'a>(
    stream: &StreamSummary,
    stream_configs: &'a [StreamConfig],
) -> Option<&'a StreamConfig> {
    stream_configs
        .iter()
        .find(|config| config.room_id == stream.room_id && config.stream_id == stream.stream_id)
}

impl Default for VoiceControlState {
    fn default() -> Self {
        Self {
            muted: false,
            deafened: false,
            push_to_talk: false,
            ptt_active: false,
            input_label: "Default microphone".to_owned(),
            output_label: "Default speaker".to_owned(),
        }
    }
}

impl Default for ScreenShareControlState {
    fn default() -> Self {
        Self {
            sharing: false,
            stream_id: 1,
            source_label: "Primary monitor".to_owned(),
            target_width: 1280,
            target_height: 720,
            target_fps: 30,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_app_selects_first_channel_when_channels_load() {
        let mut app = ClientApp::new(ClientRole::Viewer);

        app.set_channels(vec![ChannelView::new(7, "General")]);

        assert_eq!(app.selected_channel_id, Some(7));
        assert_eq!(app.selected_channel().unwrap().name, "General");
    }

    #[test]
    fn client_app_clears_missing_selected_channel() {
        let mut app = ClientApp::new(ClientRole::Viewer);
        app.set_channels(vec![ChannelView::new(1, "General")]);
        assert!(app.select_channel(1));

        app.set_channels(vec![ChannelView::new(2, "Streaming")]);

        assert_eq!(app.selected_channel_id, Some(2));
    }

    #[test]
    fn voice_controls_apply_deafen_and_push_to_talk_rules() {
        let mut app = ClientApp::new(ClientRole::Broadcaster);

        assert!(app.local_speaking());
        app.set_push_to_talk(true);
        assert!(!app.local_speaking());
        app.set_ptt_active(true);
        assert!(app.local_speaking());
        app.set_deafened(true);

        assert!(app.local_voice.deafened);
        assert!(app.local_voice.muted);
        assert!(!app.local_speaking());
    }

    #[test]
    fn channel_reports_active_screen_share_and_speakers() {
        let mut channel = ChannelView::new(3, "Live Review");
        channel.participants.push(ParticipantView {
            user_id: 9,
            display_name: "Alice".to_owned(),
            muted: false,
            deafened: false,
            push_to_talk: true,
            speaking: true,
            sharing_screen: true,
        });

        assert_eq!(channel.active_speaker_count(), 1);
        assert!(channel.screen_share_active());
    }

    #[test]
    fn channel_view_builds_selected_media_from_discovery() {
        let room = RoomSummary {
            room_id: 3,
            name: "Live Review".to_owned(),
            participant_count: 2,
            published_stream_count: 2,
        };
        let streams = vec![
            StreamSummary {
                room_id: 3,
                stream_id: 8,
                publisher_id: 9,
                codec: CodecId::H264,
                media_kind: MediaKind::Screen,
                subscriber_count: 1,
                has_config: true,
                target_bitrate_bps: 192_000,
                target_frames_per_second: 30,
            },
            StreamSummary {
                room_id: 3,
                stream_id: 9,
                publisher_id: 9,
                codec: CodecId::Opus,
                media_kind: MediaKind::Voice,
                subscriber_count: 1,
                has_config: true,
                target_bitrate_bps: 38_400,
                target_frames_per_second: 50,
            },
        ];
        let stream_configs = vec![
            StreamConfig {
                room_id: 3,
                stream_id: 8,
                codec: CodecId::H264,
                width: 1280,
                height: 720,
                frames_per_second: 30,
                timebase_hz: 90_000,
            },
            StreamConfig {
                room_id: 3,
                stream_id: 9,
                codec: CodecId::Opus,
                width: 0,
                height: 0,
                frames_per_second: 50,
                timebase_hz: 48_000,
            },
        ];
        let participants = vec![ParticipantSummary {
            room_id: 3,
            user_id: 9,
            display_name: "Alice".to_owned(),
            muted: false,
            deafened: false,
            push_to_talk: false,
            speaking: true,
            published_stream_count: 2,
            subscribed_stream_count: 0,
        }];

        let channel = ChannelView::from_discovery(&room, &streams, &stream_configs, &participants);

        assert_eq!(channel.participant_count, 1);
        assert_eq!(channel.screen_stream.as_ref().unwrap().width, 1280);
        assert_eq!(channel.voice_stream.as_ref().unwrap().frames_per_second, 50);
        assert!(channel.participants[0].sharing_screen);
    }

    #[test]
    fn client_app_serializes_camel_case_ui_state() {
        let local_voice = VoiceControlState::default();
        let local_screen_share = ScreenShareControlState::default();
        let app = ClientApp::from_discovery(ClientAppDiscovery {
            role: ClientRole::Broadcaster,
            relay_addr: "127.0.0.1:4433",
            selected_channel_id: None,
            rooms: &[RoomSummary {
                room_id: 1,
                name: "General".to_owned(),
                participant_count: 0,
                published_stream_count: 0,
            }],
            streams: &[],
            stream_configs: &[],
            participants: &[],
            local_voice: &local_voice,
            local_screen_share: &local_screen_share,
        });

        let json = serde_json::to_string(&app).unwrap();

        assert!(json.contains(r#""selectedChannelId":1"#));
        assert!(json.contains(r#""localVoice""#));
        assert!(json.contains(r#""screenStream":null"#));
        assert!(json.contains(r#""role":"broadcaster""#));
    }
}
