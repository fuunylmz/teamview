use std::collections::BTreeMap;

use teamview_protocol::{
    PROTOCOL_VERSION,
    control::{
        ClientControl, ClientEnvelope, ControlError, HelloAccepted, RoomCreated, RoomId,
        RoomJoined, RoomLeft, ServerControl, ServerEnvelope, StreamConfig, StreamId,
        StreamPublished, StreamSubscribed, StreamUnsubscribed, UserId, ViewerStatsReport,
    },
};

use crate::{
    room::{PublishedStream, Room},
    session::Session,
};

#[derive(Debug, Default)]
pub struct ControlState {
    rooms: BTreeMap<RoomId, Room>,
    viewer_stats: BTreeMap<StreamId, BTreeMap<UserId, ViewerStatsReport>>,
    next_room_id: RoomId,
    next_user_id: UserId,
}

impl ControlState {
    pub fn new() -> Self {
        Self {
            rooms: BTreeMap::new(),
            viewer_stats: BTreeMap::new(),
            next_room_id: 1,
            next_user_id: 1,
        }
    }

    pub fn handle_client_envelope(
        &mut self,
        session: &mut Session,
        envelope: ClientEnvelope,
    ) -> ServerEnvelope {
        let request_id = envelope.request_id;
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
                        session.authenticate(user_id);
                    }
                    ServerControl::HelloAccepted(HelloAccepted {
                        protocol_version: PROTOCOL_VERSION,
                        server_name: "teamview-relay".to_owned(),
                    })
                }
            }
            ClientControl::Authenticate(_) => match session.user_id {
                Some(user_id) => {
                    ServerControl::Authenticated(teamview_protocol::control::Authenticated {
                        user_id,
                        display_name: format!("user-{user_id}"),
                    })
                }
                None => ServerControl::Error(ControlError::new("not_ready", "send Hello first")),
            },
            ClientControl::CreateRoom(create) => match session.user_id {
                Some(_) => {
                    let room_id = self.next_room_id;
                    self.next_room_id += 1;
                    self.rooms.insert(room_id, Room::new(room_id, &create.name));
                    ServerControl::RoomCreated(RoomCreated {
                        room_id,
                        name: create.name,
                    })
                }
                None => {
                    ServerControl::Error(ControlError::new("unauthenticated", "send Hello first"))
                }
            },
            ClientControl::JoinRoom(join) => match self.rooms.get_mut(&join.room_id) {
                Some(room) => match session.user_id {
                    Some(user_id) => {
                        let participant_count = room.join(user_id);
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
            ClientControl::SubscribeStream(subscribe) => {
                match self.rooms.get_mut(&subscribe.room_id) {
                    Some(room) => match session.user_id {
                        Some(user_id) if room.participants.contains(&user_id) => {
                            if room.subscribe(subscribe.stream_id, user_id) {
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
                if let (Some(room), Some(user_id)) =
                    (self.rooms.get_mut(&leave.room_id), session.user_id)
                {
                    let subscribed_streams = room
                        .subscriptions
                        .iter()
                        .filter_map(|(stream_id, subscribers)| {
                            subscribers.contains(&user_id).then_some(*stream_id)
                        })
                        .collect::<Vec<_>>();
                    room.leave(user_id);
                    for stream_id in subscribed_streams {
                        self.remove_viewer_stats(stream_id, user_id);
                    }
                }
                ServerControl::RoomLeft(RoomLeft {
                    room_id: leave.room_id,
                })
            }
            ClientControl::SetStreamConfig(config) => self.set_stream_config(session, config),
            ClientControl::PollStreamConfig(poll) => {
                self.poll_stream_config(session, poll.room_id, poll.stream_id)
            }
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
            ClientControl::PollPublisherFeedback(poll) => match session.user_id {
                Some(user_id) if self.publisher_for_stream(poll.stream_id) == Some(user_id) => {
                    ServerControl::PublisherFeedback(
                        self.aggregate_publisher_feedback(poll.room_id, poll.stream_id),
                    )
                }
                Some(_) => ServerControl::Error(ControlError::new(
                    "not_publisher",
                    "only the stream publisher can poll publisher feedback",
                )),
                None => {
                    ServerControl::Error(ControlError::new("unauthenticated", "send Hello first"))
                }
            },
            ClientControl::SetTargetBitrate(_) | ClientControl::SetTargetFramerate(_) => {
                ServerControl::Error(ControlError::new(
                    "not_implemented",
                    "control accepted in later stage",
                ))
            }
        };

        ServerEnvelope::new(request_id, message)
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

    pub fn disconnect_user(&mut self, user_id: UserId) {
        for room in self.rooms.values_mut() {
            room.leave(user_id);
            let published_by_user = room
                .published_streams
                .iter()
                .filter_map(|(stream_id, stream)| {
                    (stream.publisher_id == user_id).then_some(*stream_id)
                })
                .collect::<Vec<_>>();
            for stream_id in published_by_user {
                room.published_streams.remove(&stream_id);
                room.subscriptions.remove(&stream_id);
                self.viewer_stats.remove(&stream_id);
            }
        }
        for stream_viewer_stats in self.viewer_stats.values_mut() {
            stream_viewer_stats.remove(&user_id);
        }
        self.viewer_stats
            .retain(|_, stream_viewer_stats| !stream_viewer_stats.is_empty());
    }

    fn aggregate_publisher_feedback(
        &self,
        room_id: RoomId,
        stream_id: StreamId,
    ) -> teamview_protocol::control::PublisherFeedback {
        let total_viewer_count = self.subscribers_for_stream(stream_id).len() as u32;
        let mut degraded_viewer_count = 0_u32;
        let mut keyframe_requested = false;

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
            aggregate_available_bitrate_bps: 0,
            degraded_viewer_count,
            total_viewer_count,
            keyframe_requested,
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

    fn set_stream_config(&mut self, session: &Session, config: StreamConfig) -> ServerControl {
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
        stream.config = Some(config.clone());
        ServerControl::StreamConfig(config)
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
}

fn viewer_is_degraded(report: &ViewerStatsReport) -> bool {
    report.lost_packets > 0
        || report.dropped_frames > 0
        || report.jitter_buffer_ms > 120
        || report.estimated_latency_ms > 200
}

#[cfg(test)]
mod tests {
    use teamview_protocol::{
        PROTOCOL_VERSION,
        codec::CodecId,
        control::{
            ClientEnvelope, CreateRoom, Hello, JoinRoom, MediaKind, PollPublisherFeedback,
            PollStreamConfig, PublishStream, StreamConfig, SubscribeStream, ViewerStatsReport,
        },
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
}
