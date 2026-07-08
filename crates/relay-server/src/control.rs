use std::collections::BTreeMap;

use teamview_protocol::{
    PROTOCOL_VERSION,
    control::{
        ClientControl, ClientEnvelope, ControlError, HelloAccepted, RoomCreated, RoomId,
        RoomJoined, RoomLeft, ServerControl, ServerEnvelope, StreamPublished, StreamSubscribed,
        StreamUnsubscribed, UserId,
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
            ClientControl::CreateRoom(create) => {
                let room_id = self.next_room_id;
                self.next_room_id += 1;
                self.rooms.insert(room_id, Room::new(room_id, &create.name));
                ServerControl::RoomCreated(RoomCreated {
                    room_id,
                    name: create.name,
                })
            }
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
            ClientControl::PublishStream(publish) => match self.rooms.get_mut(&publish.room_id) {
                Some(room) => match session.user_id {
                    Some(user_id) if room.participants.contains(&user_id) => {
                        room.publish_stream(PublishedStream {
                            stream_id: publish.stream_id,
                            publisher_id: user_id,
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
                None => {
                    ServerControl::Error(ControlError::new("room_not_found", "room does not exist"))
                }
            },
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
}
