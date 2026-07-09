use std::collections::BTreeMap;

use teamview_protocol::{
    PROTOCOL_VERSION,
    control::{
        ClientControl, ClientEnvelope, ControlError, HelloAccepted, RoomCreated, RoomId,
        RoomJoined, RoomLeft, ServerControl, ServerEnvelope, StreamId, StreamPublished,
        StreamSubscribed, StreamUnsubscribed, UserId,
    },
};

use crate::{
    room::{PublishedStream, Room},
    session::Session,
};

#[derive(Debug, Default)]
pub struct ControlState {
    rooms: BTreeMap<RoomId, Room>,
    next_room_id: RoomId,
    next_user_id: UserId,
}

impl ControlState {
    pub fn new() -> Self {
        Self {
            rooms: BTreeMap::new(),
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
                    room.leave(user_id);
                }
                ServerControl::RoomLeft(RoomLeft {
                    room_id: leave.room_id,
                })
            }
            ClientControl::ViewerStats(_) => {
                ServerControl::PublisherFeedback(teamview_protocol::control::PublisherFeedback {
                    room_id: 0,
                    stream_id: 0,
                    aggregate_available_bitrate_bps: 0,
                    degraded_viewer_count: 0,
                    total_viewer_count: 0,
                    keyframe_requested: false,
                })
            }
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
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use teamview_protocol::{
        PROTOCOL_VERSION,
        codec::CodecId,
        control::{
            ClientEnvelope, CreateRoom, Hello, JoinRoom, MediaKind, PublishStream, SubscribeStream,
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
}
