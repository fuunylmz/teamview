use std::collections::BTreeMap;

use teamview_protocol::{
    PROTOCOL_VERSION,
    control::{
        ClientControl, ClientEnvelope, ControlError, HelloAccepted, KeyframeReason, MediaKind,
        Pong, RequestKeyframe, RoomCreated, RoomId, RoomJoined, RoomLeft, RoomList, RoomSummary,
        ServerControl, ServerEnvelope, SetTargetBitrate, SetTargetFramerate, SetVoiceState,
        StreamConfig, StreamId, StreamList, StreamPublished, StreamSubscribed, StreamSummary,
        StreamUnsubscribed, TimeSyncResponse, UserId, ViewerStatsReport, VoiceState,
    },
    packet::MediaPacket,
};

use crate::{
    media::MediaForwardSummary,
    metrics::{StreamForwardingMetrics, unix_time_micros},
    room::{PublishedStream, Room},
    session::Session,
};

const DEFAULT_TARGET_BITRATE_BPS: u32 = 4_000_000;
const DEFAULT_TARGET_FRAMES_PER_SECOND: u16 = 30;
const MIN_TARGET_BITRATE_BPS: u32 = 16_000;
const MAX_TARGET_BITRATE_BPS: u32 = 16_000_000;
const MIN_TARGET_FRAMES_PER_SECOND: u16 = 5;
const MAX_TARGET_FRAMES_PER_SECOND: u16 = 120;
const DEGRADED_BITRATE_PERCENT: u32 = 80;
const MIN_TARGET_WIDTH: u32 = 320;
const MIN_TARGET_HEIGHT: u32 = 180;
const MAX_TARGET_WIDTH: u32 = 7680;
const MAX_TARGET_HEIGHT: u32 = 4320;

#[derive(Debug, Default)]
pub struct ControlState {
    rooms: BTreeMap<RoomId, Room>,
    viewer_stats: BTreeMap<StreamId, BTreeMap<UserId, ViewerStatsReport>>,
    voice_states: BTreeMap<RoomId, BTreeMap<UserId, VoiceState>>,
    pending_keyframe_requests: BTreeMap<StreamId, KeyframeReason>,
    stream_metrics: BTreeMap<StreamId, StreamForwardingMetrics>,
    access_token: Option<String>,
    next_room_id: RoomId,
    next_user_id: UserId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StreamTarget {
    bitrate_bps: u32,
    frames_per_second: u16,
    width: u32,
    height: u32,
}

impl ControlState {
    pub fn new() -> Self {
        Self {
            rooms: BTreeMap::new(),
            viewer_stats: BTreeMap::new(),
            voice_states: BTreeMap::new(),
            pending_keyframe_requests: BTreeMap::new(),
            stream_metrics: BTreeMap::new(),
            access_token: None,
            next_room_id: 1,
            next_user_id: 1,
        }
    }

    pub fn with_access_token(access_token: impl Into<String>) -> Self {
        let mut state = Self::new();
        state.access_token = Some(access_token.into());
        state
    }

    pub fn handle_client_envelope(
        &mut self,
        session: &mut Session,
        envelope: ClientEnvelope,
    ) -> ServerEnvelope {
        let request_id = envelope.request_id;
        if let Some(error) = self.authorization_error(session, &envelope.message) {
            return ServerEnvelope::new(request_id, error);
        }
        let message = match envelope.message {
            ClientControl::Hello(hello) => {
                if hello.protocol_version != PROTOCOL_VERSION {
                    ServerControl::Error(ControlError::new(
                        "unsupported_version",
                        format!(
                            "unsupported client protocol version {}",
                            hello.protocol_version
                        ),
                    ))
                } else {
                    if session.user_id.is_none() {
                        let user_id = self.next_user_id;
                        self.next_user_id += 1;
                        session.establish_identity(user_id, !self.requires_access_token());
                    }
                    ServerControl::HelloAccepted(HelloAccepted {
                        protocol_version: PROTOCOL_VERSION,
                        server_name: "teamview-relay".to_owned(),
                    })
                }
            }
            ClientControl::Ping(ping) => match session.user_id {
                Some(_) => ServerControl::Pong(Pong { nonce: ping.nonce }),
                None => {
                    ServerControl::Error(ControlError::new("unauthenticated", "send Hello first"))
                }
            },
            ClientControl::TimeSync(time_sync) => match session.user_id {
                Some(_) => {
                    let server_receive_time_micros = unix_time_micros();
                    ServerControl::TimeSync(TimeSyncResponse {
                        client_send_time_micros: time_sync.client_send_time_micros,
                        server_receive_time_micros,
                        server_send_time_micros: unix_time_micros(),
                    })
                }
                None => {
                    ServerControl::Error(ControlError::new("unauthenticated", "send Hello first"))
                }
            },
            ClientControl::Authenticate(authenticate) => match session.user_id {
                Some(user_id) => {
                    if let Some(expected_token) = &self.access_token
                        && authenticate.token != *expected_token
                    {
                        return ServerEnvelope::new(
                            request_id,
                            ServerControl::Error(ControlError::new(
                                "invalid_token",
                                "access token is invalid",
                            )),
                        );
                    }
                    session.grant_access();
                    ServerControl::Authenticated(teamview_protocol::control::Authenticated {
                        user_id,
                        display_name: format!("user-{user_id}"),
                    })
                }
                None => ServerControl::Error(ControlError::new("not_ready", "send Hello first")),
            },
            ClientControl::CreateRoom(create) => match session.user_id {
                Some(user_id) => {
                    let room_id = self.next_room_id;
                    self.next_room_id += 1;
                    let mut room = Room::new(room_id, &create.name);
                    room.join(user_id);
                    self.rooms.insert(room_id, room);
                    self.ensure_voice_state(room_id, user_id);
                    ServerControl::RoomCreated(RoomCreated {
                        room_id,
                        name: create.name,
                    })
                }
                None => {
                    ServerControl::Error(ControlError::new("unauthenticated", "send Hello first"))
                }
            },
            ClientControl::ListRooms(_) => ServerControl::RoomList(self.list_rooms()),
            ClientControl::JoinRoom(join) => match self.rooms.get_mut(&join.room_id) {
                Some(room) => match session.user_id {
                    Some(user_id) => {
                        let participant_count = room.join(user_id);
                        self.ensure_voice_state(join.room_id, user_id);
                        ServerControl::RoomJoined(RoomJoined {
                            room_id: join.room_id,
                            participant_count,
                        })
                    }
                    None => ServerControl::Error(ControlError::new(
                        "unauthenticated",
                        "send Hello first",
                    )),
                },
                None => {
                    ServerControl::Error(ControlError::new("room_not_found", "room does not exist"))
                }
            },
            ClientControl::PublishStream(publish) => {
                if self.publisher_for_stream(publish.stream_id).is_some() {
                    ServerControl::Error(ControlError::new(
                        "stream_id_conflict",
                        "stream id is already published",
                    ))
                } else {
                    match self.rooms.get_mut(&publish.room_id) {
                        Some(room) => match session.user_id {
                            Some(user_id) if room.participants.contains(&user_id) => {
                                room.publish_stream(PublishedStream {
                                    stream_id: publish.stream_id,
                                    publisher_id: user_id,
                                    codec: publish.codec,
                                    media_kind: publish.media_kind,
                                    config: None,
                                    target_bitrate_bps: DEFAULT_TARGET_BITRATE_BPS,
                                    target_frames_per_second: DEFAULT_TARGET_FRAMES_PER_SECOND,
                                    target_width: 0,
                                    target_height: 0,
                                });
                                ServerControl::StreamPublished(StreamPublished {
                                    room_id: publish.room_id,
                                    stream_id: publish.stream_id,
                                })
                            }
                            Some(_) => ServerControl::Error(ControlError::new(
                                "not_in_room",
                                "join room before publishing",
                            )),
                            None => ServerControl::Error(ControlError::new(
                                "unauthenticated",
                                "send Hello first",
                            )),
                        },
                        None => ServerControl::Error(ControlError::new(
                            "room_not_found",
                            "room does not exist",
                        )),
                    }
                }
            }
            ClientControl::ListStreams(list) => self.list_streams(session, list.room_id),
            ClientControl::SubscribeStream(subscribe) => {
                match self.rooms.get_mut(&subscribe.room_id) {
                    Some(room) => match session.user_id {
                        Some(user_id) if room.participants.contains(&user_id) => {
                            let already_subscribed = room
                                .subscriptions
                                .get(&subscribe.stream_id)
                                .is_some_and(|subscribers| subscribers.contains(&user_id));
                            let requests_keyframe_on_subscribe = room
                                .published_streams
                                .get(&subscribe.stream_id)
                                .is_some_and(|stream| stream.media_kind == MediaKind::Screen);
                            if room.subscribe(subscribe.stream_id, user_id) {
                                if !already_subscribed && requests_keyframe_on_subscribe {
                                    self.register_keyframe_request(
                                        subscribe.stream_id,
                                        KeyframeReason::NewSubscriber,
                                    );
                                }
                                ServerControl::StreamSubscribed(StreamSubscribed {
                                    room_id: subscribe.room_id,
                                    stream_id: subscribe.stream_id,
                                })
                            } else {
                                ServerControl::Error(ControlError::new(
                                    "stream_not_found",
                                    "stream does not exist",
                                ))
                            }
                        }
                        Some(_) => ServerControl::Error(ControlError::new(
                            "not_in_room",
                            "join room before subscribing",
                        )),
                        None => ServerControl::Error(ControlError::new(
                            "unauthenticated",
                            "send Hello first",
                        )),
                    },
                    None => ServerControl::Error(ControlError::new(
                        "room_not_found",
                        "room does not exist",
                    )),
                }
            }
            ClientControl::UnsubscribeStream(unsubscribe) => {
                if let (Some(room), Some(user_id)) =
                    (self.rooms.get_mut(&unsubscribe.room_id), session.user_id)
                {
                    room.unsubscribe(unsubscribe.stream_id, user_id);
                    self.remove_viewer_stats(unsubscribe.stream_id, user_id);
                }
                ServerControl::StreamUnsubscribed(StreamUnsubscribed {
                    room_id: unsubscribe.room_id,
                    stream_id: unsubscribe.stream_id,
                })
            }
            ClientControl::LeaveRoom(leave) => {
                if let Some(user_id) = session.user_id {
                    self.leave_room(user_id, leave.room_id);
                }
                ServerControl::RoomLeft(RoomLeft {
                    room_id: leave.room_id,
                })
            }
            ClientControl::SetVoiceState(voice_state) => self.set_voice_state(session, voice_state),
            ClientControl::SetStreamConfig(config) => self.set_stream_config(session, config),
            ClientControl::PollStreamConfig(poll) => {
                self.poll_stream_config(session, poll.room_id, poll.stream_id)
            }
            ClientControl::PollStreamMetrics(poll) => {
                self.poll_stream_metrics(session, poll.room_id, poll.stream_id)
            }
            ClientControl::RequestKeyframe(request) => self.request_keyframe(session, request),
            ClientControl::ViewerStats(report) => {
                if let Some(user_id) = session.user_id {
                    self.viewer_stats
                        .entry(report.stream_id)
                        .or_default()
                        .insert(user_id, report.clone());
                }
                ServerControl::PublisherFeedback(
                    self.aggregate_publisher_feedback(report.room_id, report.stream_id),
                )
            }
            ClientControl::PollPublisherFeedback(poll) => {
                self.poll_publisher_feedback(session, poll.room_id, poll.stream_id)
            }
            ClientControl::SetTargetBitrate(target) => self.set_target_bitrate(session, target),
            ClientControl::SetTargetFramerate(target) => self.set_target_framerate(session, target),
        };

        ServerEnvelope::new(request_id, message)
    }

    pub fn requires_access_token(&self) -> bool {
        self.access_token.is_some()
    }

    fn authorization_error(
        &self,
        session: &Session,
        message: &ClientControl,
    ) -> Option<ServerControl> {
        if matches!(
            message,
            ClientControl::Hello(_)
                | ClientControl::Ping(_)
                | ClientControl::TimeSync(_)
                | ClientControl::Authenticate(_)
        ) {
            return None;
        }
        if session.user_id.is_none() {
            return Some(ServerControl::Error(ControlError::new(
                "unauthenticated",
                "send Hello first",
            )));
        }
        if self.requires_access_token() && !session.access_granted {
            return Some(ServerControl::Error(ControlError::new(
                "not_authenticated",
                "authenticate before using relay controls",
            )));
        }
        None
    }

    pub fn room(&self, room_id: RoomId) -> Option<&Room> {
        self.rooms.get(&room_id)
    }

    pub fn published_stream(&self, stream_id: StreamId) -> Option<&PublishedStream> {
        self.rooms
            .values()
            .find_map(|room| room.published_streams.get(&stream_id))
    }

    pub fn publisher_for_stream(&self, stream_id: StreamId) -> Option<UserId> {
        self.published_stream(stream_id)
            .map(|stream| stream.publisher_id)
    }

    pub fn subscribers_for_stream(&self, stream_id: StreamId) -> Vec<UserId> {
        self.rooms
            .values()
            .filter(|room| room.published_streams.contains_key(&stream_id))
            .flat_map(|room| {
                room.subscriptions
                    .get(&stream_id)
                    .into_iter()
                    .flat_map(|subscribers| subscribers.iter().copied())
            })
            .collect()
    }

    pub fn voice_state_for_stream(
        &self,
        stream_id: StreamId,
        user_id: UserId,
    ) -> Option<&VoiceState> {
        self.rooms
            .iter()
            .find(|(_, room)| room.published_streams.contains_key(&stream_id))
            .and_then(|(room_id, _)| self.voice_state(*room_id, user_id))
    }

    pub fn voice_state(&self, room_id: RoomId, user_id: UserId) -> Option<&VoiceState> {
        self.voice_states
            .get(&room_id)
            .and_then(|room_voice_states| room_voice_states.get(&user_id))
    }

    pub fn voice_publisher_can_send(&self, stream_id: StreamId, user_id: UserId) -> bool {
        self.voice_state_for_stream(stream_id, user_id)
            .is_none_or(|voice_state| {
                !voice_state.muted && (!voice_state.push_to_talk || voice_state.speaking)
            })
    }

    pub fn disconnect_user(&mut self, user_id: UserId) {
        let affected_rooms = self
            .rooms
            .iter()
            .filter_map(|(room_id, room)| {
                let user_is_participant = room.participants.contains(&user_id);
                let user_has_stream = room
                    .published_streams
                    .values()
                    .any(|stream| stream.publisher_id == user_id);
                (user_is_participant || user_has_stream).then_some(*room_id)
            })
            .collect::<Vec<_>>();

        for room_id in affected_rooms {
            self.leave_room(user_id, room_id);
        }
        for stream_viewer_stats in self.viewer_stats.values_mut() {
            stream_viewer_stats.remove(&user_id);
        }
        self.viewer_stats
            .retain(|_, stream_viewer_stats| !stream_viewer_stats.is_empty());
    }

    fn leave_room(&mut self, user_id: UserId, room_id: RoomId) {
        let (subscribed_streams, removed_streams) = {
            let Some(room) = self.rooms.get_mut(&room_id) else {
                return;
            };
            let subscribed_streams = room
                .subscriptions
                .iter()
                .filter_map(|(stream_id, subscribers)| {
                    subscribers.contains(&user_id).then_some(*stream_id)
                })
                .collect::<Vec<_>>();
            let removed_streams = room.streams_published_by(user_id);
            room.leave(user_id);
            for stream_id in &removed_streams {
                room.remove_published_stream(*stream_id);
            }
            (subscribed_streams, removed_streams)
        };

        for stream_id in subscribed_streams {
            self.remove_viewer_stats(stream_id, user_id);
        }
        self.remove_voice_state(room_id, user_id);
        for stream_id in removed_streams {
            self.remove_stream_state(stream_id);
        }
        self.remove_room_if_empty(room_id);
    }

    fn remove_stream_state(&mut self, stream_id: StreamId) {
        self.viewer_stats.remove(&stream_id);
        self.pending_keyframe_requests.remove(&stream_id);
        self.stream_metrics.remove(&stream_id);
    }

    fn remove_room_if_empty(&mut self, room_id: RoomId) {
        if self.rooms.get(&room_id).is_some_and(Room::is_empty) {
            self.rooms.remove(&room_id);
            self.voice_states.remove(&room_id);
        }
    }

    fn ensure_voice_state(&mut self, room_id: RoomId, user_id: UserId) {
        self.voice_states
            .entry(room_id)
            .or_default()
            .entry(user_id)
            .or_insert_with(|| VoiceState {
                room_id,
                user_id,
                muted: false,
                deafened: false,
                push_to_talk: false,
                speaking: true,
            });
    }

    fn remove_voice_state(&mut self, room_id: RoomId, user_id: UserId) {
        let Some(room_voice_states) = self.voice_states.get_mut(&room_id) else {
            return;
        };
        room_voice_states.remove(&user_id);
        if room_voice_states.is_empty() {
            self.voice_states.remove(&room_id);
        }
    }

    fn set_voice_state(&mut self, session: &Session, request: SetVoiceState) -> ServerControl {
        let Some(user_id) = session.user_id else {
            return ServerControl::Error(ControlError::new("unauthenticated", "send Hello first"));
        };
        let Some(room) = self.rooms.get(&request.room_id) else {
            return ServerControl::Error(ControlError::new(
                "room_not_found",
                "room does not exist",
            ));
        };
        if !room.participants.contains(&user_id) {
            return ServerControl::Error(ControlError::new(
                "not_in_room",
                "join room before updating voice state",
            ));
        }
        let voice_state = VoiceState {
            room_id: request.room_id,
            user_id,
            muted: request.muted,
            deafened: request.deafened,
            push_to_talk: request.push_to_talk,
            speaking: request.speaking && !request.muted,
        };
        self.voice_states
            .entry(request.room_id)
            .or_default()
            .insert(user_id, voice_state.clone());
        ServerControl::VoiceStateUpdated(voice_state)
    }

    pub fn record_media_forward_summary(
        &mut self,
        packet: &MediaPacket,
        summary: MediaForwardSummary,
        ingress_bytes: usize,
        received_at_micros: u64,
        server_route_ms: u16,
    ) {
        let stream_id = packet.header.room_stream_id;
        if self.published_stream(stream_id).is_none() {
            return;
        }
        self.stream_metrics
            .entry(stream_id)
            .or_default()
            .record_forwarding(
                ingress_bytes,
                summary.queued,
                summary.dropped,
                received_at_micros,
                server_route_ms,
            );
    }

    fn aggregate_publisher_feedback(
        &self,
        room_id: RoomId,
        stream_id: StreamId,
    ) -> teamview_protocol::control::PublisherFeedback {
        let target = self.stream_target(stream_id);
        let total_viewer_count = self.subscribers_for_stream(stream_id).len() as u32;
        let mut degraded_viewer_count = 0_u32;
        let mut keyframe_requested = self.pending_keyframe_requests.contains_key(&stream_id);

        if let Some(stream_viewer_stats) = self.viewer_stats.get(&stream_id) {
            for report in stream_viewer_stats.values() {
                if viewer_is_degraded(report) {
                    degraded_viewer_count = degraded_viewer_count.saturating_add(1);
                }
                keyframe_requested |= report.lost_packets > 0 || report.dropped_frames > 0;
            }
        }

        teamview_protocol::control::PublisherFeedback {
            room_id,
            stream_id,
            aggregate_available_bitrate_bps: target.bitrate_bps,
            target_frames_per_second: target.frames_per_second,
            target_width: target.width,
            target_height: target.height,
            degraded_viewer_count,
            total_viewer_count,
            keyframe_requested,
        }
    }

    fn adaptive_publisher_feedback(
        &mut self,
        room_id: RoomId,
        stream_id: StreamId,
    ) -> teamview_protocol::control::PublisherFeedback {
        let mut feedback = self.aggregate_publisher_feedback(room_id, stream_id);
        if feedback.total_viewer_count > 0
            && feedback.degraded_viewer_count.saturating_mul(2) >= feedback.total_viewer_count
        {
            let target = self.stream_target(stream_id);
            let reduced_bitrate = target.bitrate_bps.saturating_mul(DEGRADED_BITRATE_PERCENT) / 100;
            let new_bitrate = reduced_bitrate.max(MIN_TARGET_BITRATE_BPS);
            let mut new_fps = target.frames_per_second;
            let mut new_width = target.width;
            let mut new_height = target.height;
            if new_bitrate == target.bitrate_bps
                && target.frames_per_second > MIN_TARGET_FRAMES_PER_SECOND
            {
                new_fps = target
                    .frames_per_second
                    .saturating_mul(DEGRADED_BITRATE_PERCENT as u16)
                    / 100;
                new_fps = new_fps.max(MIN_TARGET_FRAMES_PER_SECOND);
            }
            if new_bitrate == target.bitrate_bps && new_fps == target.frames_per_second {
                (new_width, new_height) = reduce_resolution(target.width, target.height);
            }
            self.update_stream_target(stream_id, new_bitrate, new_fps, new_width, new_height);
            feedback.aggregate_available_bitrate_bps = new_bitrate;
            feedback.target_frames_per_second = new_fps;
            feedback.target_width = new_width;
            feedback.target_height = new_height;
            if (new_width, new_height) != (target.width, target.height) {
                self.register_keyframe_request(stream_id, KeyframeReason::StreamConfigChanged);
                feedback.keyframe_requested = true;
            }
        }
        feedback
    }

    fn stream_target(&self, stream_id: StreamId) -> StreamTarget {
        self.published_stream(stream_id)
            .map(|stream| StreamTarget {
                bitrate_bps: stream.target_bitrate_bps,
                frames_per_second: stream.target_frames_per_second,
                width: stream.target_width,
                height: stream.target_height,
            })
            .unwrap_or(StreamTarget {
                bitrate_bps: DEFAULT_TARGET_BITRATE_BPS,
                frames_per_second: DEFAULT_TARGET_FRAMES_PER_SECOND,
                width: 0,
                height: 0,
            })
    }

    fn update_stream_target(
        &mut self,
        stream_id: StreamId,
        bitrate_bps: u32,
        frames_per_second: u16,
        width: u32,
        height: u32,
    ) {
        let Some(stream) = self
            .rooms
            .values_mut()
            .find_map(|room| room.published_streams.get_mut(&stream_id))
        else {
            return;
        };
        stream.target_bitrate_bps =
            bitrate_bps.clamp(MIN_TARGET_BITRATE_BPS, MAX_TARGET_BITRATE_BPS);
        stream.target_frames_per_second =
            frames_per_second.clamp(MIN_TARGET_FRAMES_PER_SECOND, MAX_TARGET_FRAMES_PER_SECOND);
        let (width, height) = clamp_target_resolution(width, height);
        stream.target_width = width;
        stream.target_height = height;
        if let Some(config) = &mut stream.config {
            config.frames_per_second = stream.target_frames_per_second;
            config.width = stream.target_width;
            config.height = stream.target_height;
        }
    }

    fn remove_viewer_stats(&mut self, stream_id: StreamId, user_id: UserId) {
        if let Some(stream_viewer_stats) = self.viewer_stats.get_mut(&stream_id) {
            stream_viewer_stats.remove(&user_id);
            if stream_viewer_stats.is_empty() {
                self.viewer_stats.remove(&stream_id);
            }
        }
    }

    fn register_keyframe_request(&mut self, stream_id: StreamId, reason: KeyframeReason) {
        self.pending_keyframe_requests.insert(stream_id, reason);
    }

    fn request_keyframe(&mut self, session: &Session, request: RequestKeyframe) -> ServerControl {
        let Some(user_id) = session.user_id else {
            return ServerControl::Error(ControlError::new("unauthenticated", "send Hello first"));
        };
        let Some(room) = self.rooms.get(&request.room_id) else {
            return ServerControl::Error(ControlError::new(
                "room_not_found",
                "room does not exist",
            ));
        };
        if !room.participants.contains(&user_id) {
            return ServerControl::Error(ControlError::new(
                "not_in_room",
                "join room before requesting a keyframe",
            ));
        }
        let Some(stream) = room.published_streams.get(&request.stream_id) else {
            return ServerControl::Error(ControlError::new(
                "stream_not_found",
                "stream does not exist",
            ));
        };
        let is_subscriber = room
            .subscriptions
            .get(&request.stream_id)
            .is_some_and(|subscribers| subscribers.contains(&user_id));
        if stream.publisher_id != user_id && !is_subscriber {
            return ServerControl::Error(ControlError::new(
                "not_subscribed",
                "subscribe to the stream before requesting a keyframe",
            ));
        }
        self.register_keyframe_request(request.stream_id, request.reason);
        ServerControl::RequestKeyframe(request)
    }

    fn poll_publisher_feedback(
        &mut self,
        session: &Session,
        room_id: RoomId,
        stream_id: StreamId,
    ) -> ServerControl {
        match session.user_id {
            Some(user_id) if self.publisher_for_stream(stream_id) == Some(user_id) => {
                let feedback = self.adaptive_publisher_feedback(room_id, stream_id);
                self.pending_keyframe_requests.remove(&stream_id);
                ServerControl::PublisherFeedback(feedback)
            }
            Some(_) => ServerControl::Error(ControlError::new(
                "not_publisher",
                "only the stream publisher can poll publisher feedback",
            )),
            None => ServerControl::Error(ControlError::new("unauthenticated", "send Hello first")),
        }
    }

    fn set_stream_config(&mut self, session: &Session, mut config: StreamConfig) -> ServerControl {
        let Some(user_id) = session.user_id else {
            return ServerControl::Error(ControlError::new("unauthenticated", "send Hello first"));
        };
        let Some(room) = self.rooms.get_mut(&config.room_id) else {
            return ServerControl::Error(ControlError::new(
                "room_not_found",
                "room does not exist",
            ));
        };
        let Some(stream) = room.published_streams.get_mut(&config.stream_id) else {
            return ServerControl::Error(ControlError::new(
                "stream_not_found",
                "stream does not exist",
            ));
        };
        if stream.publisher_id != user_id {
            return ServerControl::Error(ControlError::new(
                "not_publisher",
                "only the stream publisher can set stream config",
            ));
        }
        if stream.codec != config.codec {
            return ServerControl::Error(ControlError::new(
                "codec_mismatch",
                "stream config codec must match published stream codec",
            ));
        }
        stream.target_frames_per_second = config
            .frames_per_second
            .clamp(MIN_TARGET_FRAMES_PER_SECOND, MAX_TARGET_FRAMES_PER_SECOND);
        let (target_width, target_height) = clamp_target_resolution(config.width, config.height);
        stream.target_width = target_width;
        stream.target_height = target_height;
        config.frames_per_second = stream.target_frames_per_second;
        config.width = stream.target_width;
        config.height = stream.target_height;
        stream.config = Some(config.clone());
        ServerControl::StreamConfig(config)
    }

    fn set_target_bitrate(&mut self, session: &Session, target: SetTargetBitrate) -> ServerControl {
        let Some(error) =
            self.validate_publisher_control(session, target.room_id, target.stream_id)
        else {
            let current = self.stream_target(target.stream_id);
            let bitrate_bps = target
                .bitrate_bps
                .clamp(MIN_TARGET_BITRATE_BPS, MAX_TARGET_BITRATE_BPS);
            self.update_stream_target(
                target.stream_id,
                bitrate_bps,
                current.frames_per_second,
                current.width,
                current.height,
            );
            return ServerControl::PublisherFeedback(
                self.aggregate_publisher_feedback(target.room_id, target.stream_id),
            );
        };
        error
    }

    fn set_target_framerate(
        &mut self,
        session: &Session,
        target: SetTargetFramerate,
    ) -> ServerControl {
        let Some(error) =
            self.validate_publisher_control(session, target.room_id, target.stream_id)
        else {
            let current = self.stream_target(target.stream_id);
            let frames_per_second = target
                .frames_per_second
                .clamp(MIN_TARGET_FRAMES_PER_SECOND, MAX_TARGET_FRAMES_PER_SECOND);
            self.update_stream_target(
                target.stream_id,
                current.bitrate_bps,
                frames_per_second,
                current.width,
                current.height,
            );
            return ServerControl::PublisherFeedback(
                self.aggregate_publisher_feedback(target.room_id, target.stream_id),
            );
        };
        error
    }

    fn validate_publisher_control(
        &self,
        session: &Session,
        room_id: RoomId,
        stream_id: StreamId,
    ) -> Option<ServerControl> {
        let Some(user_id) = session.user_id else {
            return Some(ServerControl::Error(ControlError::new(
                "unauthenticated",
                "send Hello first",
            )));
        };
        let Some(room) = self.rooms.get(&room_id) else {
            return Some(ServerControl::Error(ControlError::new(
                "room_not_found",
                "room does not exist",
            )));
        };
        let Some(stream) = room.published_streams.get(&stream_id) else {
            return Some(ServerControl::Error(ControlError::new(
                "stream_not_found",
                "stream does not exist",
            )));
        };
        if stream.publisher_id != user_id {
            return Some(ServerControl::Error(ControlError::new(
                "not_publisher",
                "only the stream publisher can update target media settings",
            )));
        }
        None
    }

    fn poll_stream_config(
        &self,
        session: &Session,
        room_id: RoomId,
        stream_id: StreamId,
    ) -> ServerControl {
        let Some(user_id) = session.user_id else {
            return ServerControl::Error(ControlError::new("unauthenticated", "send Hello first"));
        };
        let Some(room) = self.rooms.get(&room_id) else {
            return ServerControl::Error(ControlError::new(
                "room_not_found",
                "room does not exist",
            ));
        };
        if !room.participants.contains(&user_id) {
            return ServerControl::Error(ControlError::new(
                "not_in_room",
                "join room before polling stream config",
            ));
        }
        let Some(stream) = room.published_streams.get(&stream_id) else {
            return ServerControl::Error(ControlError::new(
                "stream_not_found",
                "stream does not exist",
            ));
        };
        match &stream.config {
            Some(config) => ServerControl::StreamConfig(config.clone()),
            None => ServerControl::Error(ControlError::new(
                "stream_config_unavailable",
                "publisher has not sent stream config yet",
            )),
        }
    }

    fn list_rooms(&self) -> RoomList {
        RoomList {
            rooms: self
                .rooms
                .values()
                .map(|room| RoomSummary {
                    room_id: room.id,
                    name: room.name.clone(),
                    participant_count: room.participants.len() as u32,
                    published_stream_count: room.published_streams.len() as u32,
                })
                .collect(),
        }
    }

    fn list_streams(&self, session: &Session, room_id: RoomId) -> ServerControl {
        let Some(user_id) = session.user_id else {
            return ServerControl::Error(ControlError::new("unauthenticated", "send Hello first"));
        };
        let Some(room) = self.rooms.get(&room_id) else {
            return ServerControl::Error(ControlError::new(
                "room_not_found",
                "room does not exist",
            ));
        };
        if !room.participants.contains(&user_id) {
            return ServerControl::Error(ControlError::new(
                "not_in_room",
                "join room before listing streams",
            ));
        }
        ServerControl::StreamList(StreamList {
            room_id,
            streams: room
                .published_streams
                .values()
                .map(|stream| StreamSummary {
                    room_id,
                    stream_id: stream.stream_id,
                    publisher_id: stream.publisher_id,
                    codec: stream.codec,
                    media_kind: stream.media_kind,
                    subscriber_count: room
                        .subscriptions
                        .get(&stream.stream_id)
                        .map(|subscribers| subscribers.len() as u32)
                        .unwrap_or_default(),
                    has_config: stream.config.is_some(),
                    target_bitrate_bps: stream.target_bitrate_bps,
                    target_frames_per_second: stream.target_frames_per_second,
                })
                .collect(),
        })
    }

    fn poll_stream_metrics(
        &self,
        session: &Session,
        room_id: RoomId,
        stream_id: StreamId,
    ) -> ServerControl {
        let Some(user_id) = session.user_id else {
            return ServerControl::Error(ControlError::new("unauthenticated", "send Hello first"));
        };
        let Some(room) = self.rooms.get(&room_id) else {
            return ServerControl::Error(ControlError::new(
                "room_not_found",
                "room does not exist",
            ));
        };
        if !room.participants.contains(&user_id) {
            return ServerControl::Error(ControlError::new(
                "not_in_room",
                "join room before polling stream metrics",
            ));
        }
        if !room.published_streams.contains_key(&stream_id) {
            return ServerControl::Error(ControlError::new(
                "stream_not_found",
                "stream does not exist",
            ));
        }
        let subscriber_count = room
            .subscriptions
            .get(&stream_id)
            .map(|subscribers| subscribers.len() as u32)
            .unwrap_or_default();
        ServerControl::StreamMetrics(
            self.stream_metrics
                .get(&stream_id)
                .copied()
                .unwrap_or_default()
                .snapshot(room_id, stream_id, subscriber_count),
        )
    }
}

fn viewer_is_degraded(report: &ViewerStatsReport) -> bool {
    report.lost_packets > 0
        || report.dropped_frames > 0
        || report.jitter_buffer_ms > 120
        || viewer_latency_ms(report) > 200
        || report.reassembly_ms_p95 > 80
        || report.decode_ms_p95 > 40
        || report.render_ms_p95 > 40
        || (report.render_fps > 0 && report.render_fps < 15)
}

fn viewer_latency_ms(report: &ViewerStatsReport) -> u16 {
    if report.calibrated_latency_ms > 0 {
        report.calibrated_latency_ms
    } else {
        report.estimated_latency_ms
    }
}

fn reduce_resolution(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (width, height);
    }
    let reduced_width = width.saturating_mul(DEGRADED_BITRATE_PERCENT) / 100;
    let mut new_width = reduced_width.max(MIN_TARGET_WIDTH).min(width);
    let mut new_height = (new_width as u64 * height as u64 / width.max(1) as u64) as u32;
    new_height = new_height.max(MIN_TARGET_HEIGHT).min(height);
    if new_height == height && new_width == width {
        return (width, height);
    }
    if new_height == height {
        new_width = (new_height as u64 * width as u64 / height.max(1) as u64) as u32;
        new_width = new_width.max(MIN_TARGET_WIDTH).min(width);
    }
    clamp_target_resolution(new_width, new_height)
}

fn clamp_target_resolution(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (0, 0);
    }
    (
        width.clamp(MIN_TARGET_WIDTH, MAX_TARGET_WIDTH),
        height.clamp(MIN_TARGET_HEIGHT, MAX_TARGET_HEIGHT),
    )
}

#[cfg(test)]
mod tests {
    use teamview_protocol::{
        PROTOCOL_VERSION,
        codec::CodecId,
        control::{
            Authenticate, ClientEnvelope, CreateRoom, Hello, JoinRoom, KeyframeReason, LeaveRoom,
            ListRooms, ListStreams, MediaKind, Ping, PollPublisherFeedback, PollStreamConfig,
            PollStreamMetrics, PublishStream, RequestKeyframe, SetTargetBitrate,
            SetTargetFramerate, SetVoiceState, StreamConfig, SubscribeStream, TimeSyncRequest,
            ViewerStatsReport,
        },
        packet::{MediaPacket, MediaPacketHeader, PacketFlags, PacketType},
    };

    use super::*;

    #[test]
    fn create_room_requires_hello() {
        let mut state = ControlState::new();
        let mut session = Session::anonymous(1);

        let response = state.handle_client_envelope(
            &mut session,
            ClientEnvelope::new(
                1,
                ClientControl::CreateRoom(CreateRoom {
                    name: "stage1".to_owned(),
                }),
            ),
        );

        match response.message {
            ServerControl::Error(error) => assert_eq!(error.code, "unauthenticated"),
            other => panic!("unexpected response: {other:?}"),
        }
        assert!(state.room(1).is_none());
    }

    #[test]
    fn ping_requires_hello() {
        let mut state = ControlState::new();
        let mut session = Session::anonymous(1);

        let response = state.handle_client_envelope(
            &mut session,
            ClientEnvelope::new(1, ClientControl::Ping(Ping { nonce: 7 })),
        );

        match response.message {
            ServerControl::Error(error) => assert_eq!(error.code, "unauthenticated"),
            other => panic!("unexpected ping response: {other:?}"),
        }
    }

    #[test]
    fn ping_returns_pong_after_hello() {
        let mut state = ControlState::new();
        let mut session = Session::anonymous(1);
        authenticate(&mut state, &mut session, "client");

        let response = state.handle_client_envelope(
            &mut session,
            ClientEnvelope::new(2, ClientControl::Ping(Ping { nonce: 99 })),
        );

        assert_eq!(response.message, ServerControl::Pong(Pong { nonce: 99 }));
    }

    #[test]
    fn time_sync_requires_hello() {
        let mut state = ControlState::new();
        let mut session = Session::anonymous(1);

        let response = state.handle_client_envelope(
            &mut session,
            ClientEnvelope::new(
                1,
                ClientControl::TimeSync(TimeSyncRequest {
                    client_send_time_micros: 123,
                }),
            ),
        );

        match response.message {
            ServerControl::Error(error) => assert_eq!(error.code, "unauthenticated"),
            other => panic!("unexpected time sync response: {other:?}"),
        }
    }

    #[test]
    fn time_sync_echoes_client_time_after_hello() {
        let mut state = ControlState::new();
        let mut session = Session::anonymous(1);
        authenticate(&mut state, &mut session, "client");

        let response = state.handle_client_envelope(
            &mut session,
            ClientEnvelope::new(
                2,
                ClientControl::TimeSync(TimeSyncRequest {
                    client_send_time_micros: 1_234_567,
                }),
            ),
        );

        match response.message {
            ServerControl::TimeSync(sync) => {
                assert_eq!(sync.client_send_time_micros, 1_234_567);
                assert!(sync.server_receive_time_micros > 0);
                assert!(sync.server_send_time_micros >= sync.server_receive_time_micros);
            }
            other => panic!("unexpected time sync response: {other:?}"),
        }
    }

    #[test]
    fn access_token_blocks_controls_until_authenticate_succeeds() {
        let mut state = ControlState::with_access_token("secret");
        let mut session = Session::anonymous(1);
        authenticate(&mut state, &mut session, "client");

        let blocked = state.handle_client_envelope(
            &mut session,
            ClientEnvelope::new(
                2,
                ClientControl::CreateRoom(CreateRoom {
                    name: "stage1".to_owned(),
                }),
            ),
        );
        match blocked.message {
            ServerControl::Error(error) => assert_eq!(error.code, "not_authenticated"),
            other => panic!("unexpected unauthenticated response: {other:?}"),
        }

        let invalid = state.handle_client_envelope(
            &mut session,
            ClientEnvelope::new(
                3,
                ClientControl::Authenticate(Authenticate {
                    token: "wrong".to_owned(),
                }),
            ),
        );
        match invalid.message {
            ServerControl::Error(error) => assert_eq!(error.code, "invalid_token"),
            other => panic!("unexpected invalid token response: {other:?}"),
        }

        let authenticated = state.handle_client_envelope(
            &mut session,
            ClientEnvelope::new(
                4,
                ClientControl::Authenticate(Authenticate {
                    token: "secret".to_owned(),
                }),
            ),
        );
        assert!(matches!(
            authenticated.message,
            ServerControl::Authenticated(_)
        ));

        let room_id = create_room(&mut state, &mut session, "stage1");
        assert_eq!(room_id, 1);
    }

    #[test]
    fn duplicate_stream_id_is_rejected_across_rooms() {
        let mut state = ControlState::new();
        let mut first = Session::anonymous(1);
        let mut second = Session::anonymous(2);
        authenticate(&mut state, &mut first, "first");
        authenticate(&mut state, &mut second, "second");
        let first_room = create_room(&mut state, &mut first, "first-room");
        let second_room = create_room(&mut state, &mut second, "second-room");
        join_room(&mut state, &mut first, first_room);
        join_room(&mut state, &mut second, second_room);
        publish_stream(&mut state, &mut first, first_room, 9);

        let duplicate = state.handle_client_envelope(
            &mut second,
            ClientEnvelope::new(
                7,
                ClientControl::PublishStream(PublishStream {
                    room_id: second_room,
                    stream_id: 9,
                    codec: CodecId::H264,
                    media_kind: MediaKind::Screen,
                }),
            ),
        );

        match duplicate.message {
            ServerControl::Error(error) => assert_eq!(error.code, "stream_id_conflict"),
            other => panic!("unexpected duplicate stream response: {other:?}"),
        }
    }

    #[test]
    fn create_room_adds_creator_as_participant() {
        let mut state = ControlState::new();
        let mut session = Session::anonymous(1);
        authenticate(&mut state, &mut session, "creator");

        let room_id = create_room(&mut state, &mut session, "stage1");

        let room = state.room(room_id).unwrap();
        assert!(room.participants.contains(&session.user_id.unwrap()));
        assert_eq!(room.participants.len(), 1);
    }

    #[test]
    fn list_rooms_returns_room_summaries() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        create_room(&mut state, &mut publisher, "empty");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);

        let response = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(6, ClientControl::ListRooms(ListRooms)),
        );

        match response.message {
            ServerControl::RoomList(list) => {
                assert_eq!(list.rooms.len(), 2);
                assert_eq!(list.rooms[0].room_id, room_id);
                assert_eq!(list.rooms[0].name, "stage1");
                assert_eq!(list.rooms[0].participant_count, 2);
                assert_eq!(list.rooms[0].published_stream_count, 1);
                assert_eq!(list.rooms[1].name, "empty");
                assert_eq!(list.rooms[1].participant_count, 1);
                assert_eq!(list.rooms[1].published_stream_count, 0);
            }
            other => panic!("unexpected room list response: {other:?}"),
        }
    }

    #[test]
    fn room_participant_can_list_streams() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);
        subscribe_stream(&mut state, &mut viewer, room_id, 9);
        let config = sample_stream_config(room_id, 9);
        state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(6, ClientControl::SetStreamConfig(config)),
        );

        let response = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(7, ClientControl::ListStreams(ListStreams { room_id })),
        );

        match response.message {
            ServerControl::StreamList(list) => {
                assert_eq!(list.room_id, room_id);
                assert_eq!(list.streams.len(), 1);
                let stream = &list.streams[0];
                assert_eq!(stream.room_id, room_id);
                assert_eq!(stream.stream_id, 9);
                assert_eq!(stream.publisher_id, publisher.user_id.unwrap());
                assert_eq!(stream.codec, CodecId::H264);
                assert_eq!(stream.media_kind, MediaKind::Screen);
                assert_eq!(stream.subscriber_count, 1);
                assert!(stream.has_config);
                assert_eq!(stream.target_bitrate_bps, DEFAULT_TARGET_BITRATE_BPS);
                assert_eq!(
                    stream.target_frames_per_second,
                    DEFAULT_TARGET_FRAMES_PER_SECOND
                );
                let published = state.published_stream(9).unwrap();
                assert_eq!(published.target_width, 1280);
                assert_eq!(published.target_height, 720);
            }
            other => panic!("unexpected stream list response: {other:?}"),
        }
    }

    #[test]
    fn non_participant_cannot_list_streams() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut outsider = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut outsider, "outsider");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);

        let response = state.handle_client_envelope(
            &mut outsider,
            ClientEnvelope::new(6, ClientControl::ListStreams(ListStreams { room_id })),
        );

        match response.message {
            ServerControl::Error(error) => assert_eq!(error.code, "not_in_room"),
            other => panic!("unexpected stream list response: {other:?}"),
        }
    }

    #[test]
    fn disconnect_user_removes_participation_subscriptions_and_streams() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);
        subscribe_stream(&mut state, &mut viewer, room_id, 9);

        state.disconnect_user(publisher.user_id.unwrap());

        let room = state.room(room_id).unwrap();
        assert!(!room.participants.contains(&publisher.user_id.unwrap()));
        assert!(!room.published_streams.contains_key(&9));
        assert!(!room.subscriptions.contains_key(&9));
    }

    #[test]
    fn disconnect_user_removes_empty_rooms_from_discovery() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);

        state.disconnect_user(publisher.user_id.unwrap());

        assert!(state.room(room_id).is_none());
        let response = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(6, ClientControl::ListRooms(ListRooms)),
        );
        match response.message {
            ServerControl::RoomList(list) => assert!(list.rooms.is_empty()),
            other => panic!("unexpected room list response: {other:?}"),
        }
    }

    #[test]
    fn publisher_leave_removes_owned_streams_and_empty_room() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        authenticate(&mut state, &mut publisher, "publisher");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);

        let response = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(6, ClientControl::LeaveRoom(LeaveRoom { room_id })),
        );

        assert_eq!(
            response.message,
            ServerControl::RoomLeft(RoomLeft { room_id })
        );
        assert!(state.room(room_id).is_none());
        assert!(state.published_stream(9).is_none());
    }

    #[test]
    fn viewer_leave_keeps_published_room_and_removes_subscription_stats() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);
        subscribe_stream(&mut state, &mut viewer, room_id, 9);
        state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                6,
                ClientControl::ViewerStats(ViewerStatsReport {
                    room_id,
                    stream_id: 9,
                    received_packets: 10,
                    lost_packets: 0,
                    decoded_frames: 2,
                    dropped_frames: 0,
                    jitter_buffer_ms: 40,
                    estimated_latency_ms: 90,
                    ..viewer_stats_report(room_id, 9)
                }),
            ),
        );

        let response = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(7, ClientControl::LeaveRoom(LeaveRoom { room_id })),
        );

        assert_eq!(
            response.message,
            ServerControl::RoomLeft(RoomLeft { room_id })
        );
        let room = state.room(room_id).unwrap();
        assert!(room.published_streams.contains_key(&9));
        assert!(!room.participants.contains(&viewer.user_id.unwrap()));
        assert!(!room.subscriptions.contains_key(&9));
        assert!(
            !state
                .viewer_stats
                .get(&9)
                .is_some_and(|stats| stats.contains_key(&viewer.user_id.unwrap()))
        );
    }

    #[test]
    fn publisher_can_poll_recorded_stream_metrics() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);
        subscribe_stream(&mut state, &mut viewer, room_id, 9);

        let packet = synthetic_packet(9);
        let ingress_bytes = packet.encode().unwrap().len();
        state.record_media_forward_summary(
            &packet,
            MediaForwardSummary {
                stream_id: 9,
                queued: 1,
                dropped: 1,
            },
            ingress_bytes,
            1_700_000,
            4,
        );

        let response = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                6,
                ClientControl::PollStreamMetrics(PollStreamMetrics {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );

        match response.message {
            ServerControl::StreamMetrics(metrics) => {
                assert_eq!(metrics.room_id, room_id);
                assert_eq!(metrics.stream_id, 9);
                assert_eq!(metrics.ingress_packets, 1);
                assert_eq!(metrics.ingress_bytes, ingress_bytes as u64);
                assert_eq!(metrics.egress_queued_packets, 1);
                assert_eq!(metrics.egress_dropped_packets, 1);
                assert_eq!(metrics.egress_queue_packets, 0);
                assert_eq!(metrics.egress_queue_media_ms, 0);
                assert_eq!(metrics.subscriber_count, 1);
                assert_eq!(metrics.last_ingress_time_micros, 1_700_000);
                assert_eq!(metrics.server_route_ms_p50, 4);
                assert_eq!(metrics.server_route_ms_p95, 4);
            }
            other => panic!("unexpected stream metrics response: {other:?}"),
        }
    }

    #[test]
    fn non_participant_cannot_poll_stream_metrics() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut outsider = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut outsider, "outsider");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);

        let response = state.handle_client_envelope(
            &mut outsider,
            ClientEnvelope::new(
                6,
                ClientControl::PollStreamMetrics(PollStreamMetrics {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );

        match response.message {
            ServerControl::Error(error) => assert_eq!(error.code, "not_in_room"),
            other => panic!("unexpected stream metrics response: {other:?}"),
        }
    }

    #[test]
    fn viewer_stats_returns_degraded_publisher_feedback() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);
        subscribe_stream(&mut state, &mut viewer, room_id, 9);

        let feedback = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                6,
                ClientControl::ViewerStats(ViewerStatsReport {
                    room_id,
                    stream_id: 9,
                    received_packets: 10,
                    lost_packets: 1,
                    decoded_frames: 2,
                    dropped_frames: 0,
                    jitter_buffer_ms: 40,
                    estimated_latency_ms: 90,
                    ..viewer_stats_report(room_id, 9)
                }),
            ),
        );

        match feedback.message {
            ServerControl::PublisherFeedback(feedback) => {
                assert_eq!(feedback.room_id, room_id);
                assert_eq!(feedback.stream_id, 9);
                assert_eq!(feedback.total_viewer_count, 1);
                assert_eq!(feedback.degraded_viewer_count, 1);
                assert!(feedback.keyframe_requested);
            }
            other => panic!("unexpected viewer stats response: {other:?}"),
        }
    }

    #[test]
    fn slow_decode_or_render_marks_viewer_degraded() {
        let mut report = viewer_stats_report(1, 9);
        report.decode_ms_p95 = 45;
        assert!(viewer_is_degraded(&report));

        let mut report = viewer_stats_report(1, 9);
        report.reassembly_ms_p95 = 90;
        assert!(viewer_is_degraded(&report));

        let mut report = viewer_stats_report(1, 9);
        report.render_ms_p95 = 45;
        assert!(viewer_is_degraded(&report));

        let mut report = viewer_stats_report(1, 9);
        report.render_fps = 12;
        assert!(viewer_is_degraded(&report));

        let mut report = viewer_stats_report(1, 9);
        report.render_fps = 30;
        assert!(!viewer_is_degraded(&report));
    }

    #[test]
    fn calibrated_latency_is_used_for_viewer_degradation() {
        let mut report = viewer_stats_report(1, 9);
        report.estimated_latency_ms = 0;
        report.calibrated_latency_ms = 220;
        assert!(viewer_is_degraded(&report));

        let mut report = viewer_stats_report(1, 9);
        report.estimated_latency_ms = 220;
        report.calibrated_latency_ms = 90;
        assert!(!viewer_is_degraded(&report));

        let mut report = viewer_stats_report(1, 9);
        report.estimated_latency_ms = 220;
        report.calibrated_latency_ms = 0;
        assert!(viewer_is_degraded(&report));
    }

    #[test]
    fn publisher_can_poll_aggregated_viewer_feedback() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut first_viewer = Session::anonymous(2);
        let mut second_viewer = Session::anonymous(3);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut first_viewer, "first-viewer");
        authenticate(&mut state, &mut second_viewer, "second-viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut first_viewer, room_id);
        join_room(&mut state, &mut second_viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);
        subscribe_stream(&mut state, &mut first_viewer, room_id, 9);
        subscribe_stream(&mut state, &mut second_viewer, room_id, 9);

        state.handle_client_envelope(
            &mut first_viewer,
            ClientEnvelope::new(
                6,
                ClientControl::ViewerStats(ViewerStatsReport {
                    room_id,
                    stream_id: 9,
                    received_packets: 10,
                    lost_packets: 0,
                    decoded_frames: 5,
                    dropped_frames: 0,
                    jitter_buffer_ms: 40,
                    estimated_latency_ms: 90,
                    ..viewer_stats_report(room_id, 9)
                }),
            ),
        );
        state.handle_client_envelope(
            &mut second_viewer,
            ClientEnvelope::new(
                7,
                ClientControl::ViewerStats(ViewerStatsReport {
                    room_id,
                    stream_id: 9,
                    received_packets: 9,
                    lost_packets: 1,
                    decoded_frames: 4,
                    dropped_frames: 1,
                    jitter_buffer_ms: 130,
                    estimated_latency_ms: 220,
                    ..viewer_stats_report(room_id, 9)
                }),
            ),
        );

        let feedback = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                8,
                ClientControl::PollPublisherFeedback(PollPublisherFeedback {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );

        match feedback.message {
            ServerControl::PublisherFeedback(feedback) => {
                assert_eq!(feedback.total_viewer_count, 2);
                assert_eq!(feedback.degraded_viewer_count, 1);
                assert!(feedback.keyframe_requested);
            }
            other => panic!("unexpected poll feedback response: {other:?}"),
        }
    }

    #[test]
    fn publisher_sets_target_media_settings() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        authenticate(&mut state, &mut publisher, "publisher");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);

        let bitrate = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                6,
                ClientControl::SetTargetBitrate(SetTargetBitrate {
                    room_id,
                    stream_id: 9,
                    bitrate_bps: 96_000,
                }),
            ),
        );
        match bitrate.message {
            ServerControl::PublisherFeedback(feedback) => {
                assert_eq!(feedback.aggregate_available_bitrate_bps, 96_000);
                assert_eq!(feedback.target_frames_per_second, 30);
                assert_eq!(feedback.target_width, 0);
                assert_eq!(feedback.target_height, 0);
            }
            other => panic!("unexpected target bitrate response: {other:?}"),
        }

        let framerate = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                7,
                ClientControl::SetTargetFramerate(SetTargetFramerate {
                    room_id,
                    stream_id: 9,
                    frames_per_second: 12,
                }),
            ),
        );
        match framerate.message {
            ServerControl::PublisherFeedback(feedback) => {
                assert_eq!(feedback.aggregate_available_bitrate_bps, 96_000);
                assert_eq!(feedback.target_frames_per_second, 12);
                assert_eq!(feedback.target_width, 0);
                assert_eq!(feedback.target_height, 0);
            }
            other => panic!("unexpected target framerate response: {other:?}"),
        }
    }

    #[test]
    fn non_publisher_cannot_set_target_media_settings() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);

        let response = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                6,
                ClientControl::SetTargetBitrate(SetTargetBitrate {
                    room_id,
                    stream_id: 9,
                    bitrate_bps: 96_000,
                }),
            ),
        );

        match response.message {
            ServerControl::Error(error) => assert_eq!(error.code, "not_publisher"),
            other => panic!("unexpected set target response: {other:?}"),
        }
    }

    #[test]
    fn degraded_majority_reduces_target_bitrate() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);
        subscribe_stream(&mut state, &mut viewer, room_id, 9);
        state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                6,
                ClientControl::SetTargetBitrate(SetTargetBitrate {
                    room_id,
                    stream_id: 9,
                    bitrate_bps: 100_000,
                }),
            ),
        );
        state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                7,
                ClientControl::ViewerStats(ViewerStatsReport {
                    room_id,
                    stream_id: 9,
                    received_packets: 10,
                    lost_packets: 2,
                    decoded_frames: 4,
                    dropped_frames: 1,
                    jitter_buffer_ms: 160,
                    estimated_latency_ms: 240,
                    ..viewer_stats_report(room_id, 9)
                }),
            ),
        );

        let feedback = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                8,
                ClientControl::PollPublisherFeedback(PollPublisherFeedback {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );

        match feedback.message {
            ServerControl::PublisherFeedback(feedback) => {
                assert_eq!(feedback.aggregate_available_bitrate_bps, 80_000);
                assert_eq!(feedback.degraded_viewer_count, 1);
                assert!(feedback.keyframe_requested);
            }
            other => panic!("unexpected degraded feedback response: {other:?}"),
        }
    }

    #[test]
    fn degraded_majority_reduces_resolution_after_bitrate_and_fps_floor() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);
        state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                6,
                ClientControl::SetStreamConfig(sample_stream_config(room_id, 9)),
            ),
        );
        subscribe_stream(&mut state, &mut viewer, room_id, 9);
        state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                7,
                ClientControl::SetTargetBitrate(SetTargetBitrate {
                    room_id,
                    stream_id: 9,
                    bitrate_bps: MIN_TARGET_BITRATE_BPS,
                }),
            ),
        );
        state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                8,
                ClientControl::SetTargetFramerate(SetTargetFramerate {
                    room_id,
                    stream_id: 9,
                    frames_per_second: MIN_TARGET_FRAMES_PER_SECOND,
                }),
            ),
        );
        state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                9,
                ClientControl::ViewerStats(ViewerStatsReport {
                    room_id,
                    stream_id: 9,
                    received_packets: 10,
                    lost_packets: 0,
                    decoded_frames: 4,
                    dropped_frames: 0,
                    jitter_buffer_ms: 160,
                    estimated_latency_ms: 240,
                    ..viewer_stats_report(room_id, 9)
                }),
            ),
        );

        let feedback = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                10,
                ClientControl::PollPublisherFeedback(PollPublisherFeedback {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );

        match feedback.message {
            ServerControl::PublisherFeedback(feedback) => {
                assert_eq!(
                    feedback.aggregate_available_bitrate_bps,
                    MIN_TARGET_BITRATE_BPS
                );
                assert_eq!(
                    feedback.target_frames_per_second,
                    MIN_TARGET_FRAMES_PER_SECOND
                );
                assert_eq!(feedback.target_width, 1024);
                assert_eq!(feedback.target_height, 576);
                assert!(feedback.keyframe_requested);
                let config = state.published_stream(9).unwrap().config.as_ref().unwrap();
                assert_eq!(config.width, 1024);
                assert_eq!(config.height, 576);
            }
            other => panic!("unexpected degraded feedback response: {other:?}"),
        }
    }

    #[test]
    fn new_subscriber_requests_keyframe_until_publisher_polls_feedback() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);
        subscribe_stream(&mut state, &mut viewer, room_id, 9);

        let first = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                6,
                ClientControl::PollPublisherFeedback(PollPublisherFeedback {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );
        match first.message {
            ServerControl::PublisherFeedback(feedback) => {
                assert_eq!(feedback.total_viewer_count, 1);
                assert!(feedback.keyframe_requested);
            }
            other => panic!("unexpected first feedback response: {other:?}"),
        }

        let second = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                7,
                ClientControl::PollPublisherFeedback(PollPublisherFeedback {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );
        match second.message {
            ServerControl::PublisherFeedback(feedback) => {
                assert_eq!(feedback.total_viewer_count, 1);
                assert!(!feedback.keyframe_requested);
            }
            other => panic!("unexpected second feedback response: {other:?}"),
        }
    }

    #[test]
    fn voice_subscriber_does_not_request_keyframe() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_voice_stream(&mut state, &mut publisher, room_id, 9);
        subscribe_stream(&mut state, &mut viewer, room_id, 9);

        let response = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                6,
                ClientControl::PollPublisherFeedback(PollPublisherFeedback {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );

        match response.message {
            ServerControl::PublisherFeedback(feedback) => {
                assert_eq!(feedback.total_viewer_count, 1);
                assert!(!feedback.keyframe_requested);
            }
            other => panic!("unexpected voice feedback response: {other:?}"),
        }
    }

    #[test]
    fn subscribed_viewer_can_request_decoder_recovery_keyframe() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);
        subscribe_stream(&mut state, &mut viewer, room_id, 9);
        state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                6,
                ClientControl::PollPublisherFeedback(PollPublisherFeedback {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );

        let request = RequestKeyframe {
            room_id,
            stream_id: 9,
            reason: KeyframeReason::DecoderRecovery,
        };
        let acknowledged = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(7, ClientControl::RequestKeyframe(request.clone())),
        );
        assert_eq!(
            acknowledged.message,
            ServerControl::RequestKeyframe(request)
        );

        let feedback = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                8,
                ClientControl::PollPublisherFeedback(PollPublisherFeedback {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );
        match feedback.message {
            ServerControl::PublisherFeedback(feedback) => assert!(feedback.keyframe_requested),
            other => panic!("unexpected feedback response: {other:?}"),
        }
    }

    #[test]
    fn non_subscriber_cannot_request_keyframe() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);

        let response = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                6,
                ClientControl::RequestKeyframe(RequestKeyframe {
                    room_id,
                    stream_id: 9,
                    reason: KeyframeReason::DecoderRecovery,
                }),
            ),
        );

        match response.message {
            ServerControl::Error(error) => assert_eq!(error.code, "not_subscribed"),
            other => panic!("unexpected request-keyframe response: {other:?}"),
        }
    }

    #[test]
    fn non_publisher_cannot_poll_publisher_feedback() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);

        let response = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                6,
                ClientControl::PollPublisherFeedback(PollPublisherFeedback {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );

        match response.message {
            ServerControl::Error(error) => assert_eq!(error.code, "not_publisher"),
            other => panic!("unexpected non-publisher poll response: {other:?}"),
        }
    }

    #[test]
    fn publisher_sets_and_viewer_polls_stream_config() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);
        subscribe_stream(&mut state, &mut viewer, room_id, 9);

        let config = sample_stream_config(room_id, 9);
        let set = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(6, ClientControl::SetStreamConfig(config.clone())),
        );
        assert_eq!(set.message, ServerControl::StreamConfig(config.clone()));

        let polled = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                7,
                ClientControl::PollStreamConfig(PollStreamConfig {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );
        assert_eq!(polled.message, ServerControl::StreamConfig(config));
    }

    #[test]
    fn stream_config_poll_reports_unavailable_until_publisher_sets_it() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);
        subscribe_stream(&mut state, &mut viewer, room_id, 9);

        let response = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                6,
                ClientControl::PollStreamConfig(PollStreamConfig {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );

        match response.message {
            ServerControl::Error(error) => assert_eq!(error.code, "stream_config_unavailable"),
            other => panic!("unexpected config response: {other:?}"),
        }
    }

    #[test]
    fn non_publisher_cannot_set_stream_config() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        join_room(&mut state, &mut viewer, room_id);
        publish_stream(&mut state, &mut publisher, room_id, 9);

        let response = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                6,
                ClientControl::SetStreamConfig(sample_stream_config(room_id, 9)),
            ),
        );

        match response.message {
            ServerControl::Error(error) => assert_eq!(error.code, "not_publisher"),
            other => panic!("unexpected set config response: {other:?}"),
        }
    }

    #[test]
    fn create_join_publish_subscribe_flow_updates_state() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);

        let accepted = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                1,
                ClientControl::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "publisher".to_owned(),
                }),
            ),
        );
        assert!(matches!(accepted.message, ServerControl::HelloAccepted(_)));

        state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                1,
                ClientControl::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "viewer".to_owned(),
                }),
            ),
        );

        let created = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                2,
                ClientControl::CreateRoom(CreateRoom {
                    name: "stage1".to_owned(),
                }),
            ),
        );
        let room_id = match created.message {
            ServerControl::RoomCreated(room) => room.room_id,
            other => panic!("unexpected response: {other:?}"),
        };

        state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(3, ClientControl::JoinRoom(JoinRoom { room_id })),
        );
        state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(3, ClientControl::JoinRoom(JoinRoom { room_id })),
        );
        state.handle_client_envelope(
            &mut publisher,
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
        let subscribed = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                5,
                ClientControl::SubscribeStream(SubscribeStream {
                    room_id,
                    stream_id: 9,
                }),
            ),
        );

        assert!(matches!(
            subscribed.message,
            ServerControl::StreamSubscribed(_)
        ));
        let room = state.room(room_id).unwrap();
        assert_eq!(room.participants.len(), 2);
        assert_eq!(room.published_streams.len(), 1);
        assert_eq!(room.subscriptions.get(&9).unwrap().len(), 1);
    }

    #[test]
    fn voice_state_update_requires_room_participant() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        let mut viewer = Session::anonymous(2);
        authenticate(&mut state, &mut publisher, "publisher");
        authenticate(&mut state, &mut viewer, "viewer");
        let room_id = create_room(&mut state, &mut publisher, "stage1");

        let response = state.handle_client_envelope(
            &mut viewer,
            ClientEnvelope::new(
                4,
                ClientControl::SetVoiceState(SetVoiceState {
                    room_id,
                    muted: true,
                    deafened: false,
                    push_to_talk: false,
                    speaking: false,
                }),
            ),
        );

        match response.message {
            ServerControl::Error(error) => assert_eq!(error.code, "not_in_room"),
            other => panic!("unexpected voice state response: {other:?}"),
        }
    }

    #[test]
    fn voice_state_update_is_stored() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        authenticate(&mut state, &mut publisher, "publisher");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        let user_id = publisher.user_id.unwrap();

        let response = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                4,
                ClientControl::SetVoiceState(SetVoiceState {
                    room_id,
                    muted: true,
                    deafened: true,
                    push_to_talk: true,
                    speaking: false,
                }),
            ),
        );

        assert_eq!(
            response.message,
            ServerControl::VoiceStateUpdated(VoiceState {
                room_id,
                user_id,
                muted: true,
                deafened: true,
                push_to_talk: true,
                speaking: false,
            })
        );
        assert_eq!(
            state.voice_state(room_id, user_id),
            Some(&VoiceState {
                room_id,
                user_id,
                muted: true,
                deafened: true,
                push_to_talk: true,
                speaking: false,
            })
        );
    }

    #[test]
    fn muted_voice_state_clears_speaking() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        authenticate(&mut state, &mut publisher, "publisher");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        let user_id = publisher.user_id.unwrap();

        let response = state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                4,
                ClientControl::SetVoiceState(SetVoiceState {
                    room_id,
                    muted: true,
                    deafened: false,
                    push_to_talk: true,
                    speaking: true,
                }),
            ),
        );

        assert_eq!(
            response.message,
            ServerControl::VoiceStateUpdated(VoiceState {
                room_id,
                user_id,
                muted: true,
                deafened: false,
                push_to_talk: true,
                speaking: false,
            })
        );
    }

    #[test]
    fn leaving_room_removes_voice_state() {
        let mut state = ControlState::new();
        let mut publisher = Session::anonymous(1);
        authenticate(&mut state, &mut publisher, "publisher");
        let room_id = create_room(&mut state, &mut publisher, "stage1");
        join_room(&mut state, &mut publisher, room_id);
        let user_id = publisher.user_id.unwrap();

        state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(
                4,
                ClientControl::SetVoiceState(SetVoiceState {
                    room_id,
                    muted: true,
                    deafened: false,
                    push_to_talk: false,
                    speaking: false,
                }),
            ),
        );
        assert!(state.voice_state(room_id, user_id).is_some());

        state.handle_client_envelope(
            &mut publisher,
            ClientEnvelope::new(5, ClientControl::LeaveRoom(LeaveRoom { room_id })),
        );

        assert!(state.voice_state(room_id, user_id).is_none());
    }

    fn authenticate(state: &mut ControlState, session: &mut Session, client_name: &str) {
        let response = state.handle_client_envelope(
            session,
            ClientEnvelope::new(
                1,
                ClientControl::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: client_name.to_owned(),
                }),
            ),
        );
        assert!(matches!(response.message, ServerControl::HelloAccepted(_)));
    }

    fn create_room(state: &mut ControlState, session: &mut Session, name: &str) -> RoomId {
        let response = state.handle_client_envelope(
            session,
            ClientEnvelope::new(
                2,
                ClientControl::CreateRoom(CreateRoom {
                    name: name.to_owned(),
                }),
            ),
        );
        match response.message {
            ServerControl::RoomCreated(room) => room.room_id,
            other => panic!("unexpected create room response: {other:?}"),
        }
    }

    fn join_room(state: &mut ControlState, session: &mut Session, room_id: RoomId) {
        let response = state.handle_client_envelope(
            session,
            ClientEnvelope::new(3, ClientControl::JoinRoom(JoinRoom { room_id })),
        );
        assert!(matches!(response.message, ServerControl::RoomJoined(_)));
    }

    fn publish_stream(
        state: &mut ControlState,
        session: &mut Session,
        room_id: RoomId,
        stream_id: StreamId,
    ) {
        let response = state.handle_client_envelope(
            session,
            ClientEnvelope::new(
                4,
                ClientControl::PublishStream(PublishStream {
                    room_id,
                    stream_id,
                    codec: CodecId::H264,
                    media_kind: MediaKind::Screen,
                }),
            ),
        );
        assert!(matches!(
            response.message,
            ServerControl::StreamPublished(_)
        ));
    }

    fn publish_voice_stream(
        state: &mut ControlState,
        session: &mut Session,
        room_id: RoomId,
        stream_id: StreamId,
    ) {
        let response = state.handle_client_envelope(
            session,
            ClientEnvelope::new(
                4,
                ClientControl::PublishStream(PublishStream {
                    room_id,
                    stream_id,
                    codec: CodecId::Opus,
                    media_kind: MediaKind::Voice,
                }),
            ),
        );
        assert!(matches!(
            response.message,
            ServerControl::StreamPublished(_)
        ));
    }

    fn subscribe_stream(
        state: &mut ControlState,
        session: &mut Session,
        room_id: RoomId,
        stream_id: StreamId,
    ) {
        let response = state.handle_client_envelope(
            session,
            ClientEnvelope::new(
                5,
                ClientControl::SubscribeStream(SubscribeStream { room_id, stream_id }),
            ),
        );
        assert!(matches!(
            response.message,
            ServerControl::StreamSubscribed(_)
        ));
    }

    fn viewer_stats_report(room_id: RoomId, stream_id: StreamId) -> ViewerStatsReport {
        ViewerStatsReport {
            room_id,
            stream_id,
            received_packets: 0,
            lost_packets: 0,
            decoded_frames: 0,
            dropped_frames: 0,
            jitter_buffer_ms: 0,
            estimated_latency_ms: 0,
            calibrated_latency_ms: 0,
            reassembly_ms_p50: 0,
            reassembly_ms_p95: 0,
            decode_ms_p50: 0,
            decode_ms_p95: 0,
            render_ms_p50: 0,
            render_ms_p95: 0,
            render_fps: 0,
        }
    }

    fn sample_stream_config(room_id: RoomId, stream_id: StreamId) -> StreamConfig {
        StreamConfig {
            room_id,
            stream_id,
            codec: CodecId::H264,
            width: 1280,
            height: 720,
            frames_per_second: 30,
            timebase_hz: 90_000,
        }
    }

    fn synthetic_packet(stream_id: StreamId) -> MediaPacket {
        let payload = bytes::Bytes::from_static(b"synthetic");
        let mut header = MediaPacketHeader::new(
            PacketType::Video,
            CodecId::H264,
            stream_id,
            1,
            payload.len() as u16,
        );
        header.frame_id = 1;
        header.flags = PacketFlags::empty().with(PacketFlags::END_OF_FRAME);
        MediaPacket { header, payload }
    }
}
