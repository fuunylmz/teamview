use teamview_protocol::control::{RoomId as ChannelId, StreamId, UserId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientRole {
    Broadcaster,
    Viewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelView {
    pub channel_id: ChannelId,
    pub name: String,
    pub participants: Vec<ParticipantView>,
    pub screen_stream: Option<ScreenStreamView>,
    pub voice_stream: Option<VoiceStreamView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantView {
    pub user_id: UserId,
    pub display_name: String,
    pub muted: bool,
    pub deafened: bool,
    pub push_to_talk: bool,
    pub speaking: bool,
    pub sharing_screen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenStreamView {
    pub stream_id: StreamId,
    pub publisher_id: UserId,
    pub width: u32,
    pub height: u32,
    pub frames_per_second: u16,
    pub subscribed: bool,
    pub rendered_frames: u64,
    pub dropped_frames: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceStreamView {
    pub stream_id: StreamId,
    pub publisher_id: UserId,
    pub subscribed: bool,
    pub decoded_frames: u64,
    pub dropped_frames: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceControlState {
    pub muted: bool,
    pub deafened: bool,
    pub push_to_talk: bool,
    pub ptt_active: bool,
    pub input_label: String,
    pub output_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenShareControlState {
    pub sharing: bool,
    pub stream_id: StreamId,
    pub source_label: String,
    pub target_width: u32,
    pub target_height: u32,
    pub target_fps: u16,
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
            participants: Vec::new(),
            screen_stream: None,
            voice_stream: None,
        }
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
}
