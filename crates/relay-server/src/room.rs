use std::collections::{BTreeMap, BTreeSet};

use teamview_protocol::control::{RoomId, StreamId, UserId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub id: RoomId,
    pub name: String,
    pub participants: BTreeSet<UserId>,
    pub published_streams: BTreeMap<StreamId, PublishedStream>,
    pub subscriptions: BTreeMap<StreamId, BTreeSet<UserId>>,
}

impl Room {
    pub fn new(id: RoomId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            participants: BTreeSet::new(),
            published_streams: BTreeMap::new(),
            subscriptions: BTreeMap::new(),
        }
    }

    pub fn join(&mut self, user_id: UserId) -> u32 {
        self.participants.insert(user_id);
        self.participants.len() as u32
    }

    pub fn leave(&mut self, user_id: UserId) {
        self.participants.remove(&user_id);
        for subscribers in self.subscriptions.values_mut() {
            subscribers.remove(&user_id);
        }
    }

    pub fn publish_stream(&mut self, stream: PublishedStream) {
        self.published_streams.insert(stream.stream_id, stream);
    }

    pub fn subscribe(&mut self, stream_id: StreamId, user_id: UserId) -> bool {
        if !self.published_streams.contains_key(&stream_id) {
            return false;
        }
        self.subscriptions
            .entry(stream_id)
            .or_default()
            .insert(user_id);
        true
    }

    pub fn unsubscribe(&mut self, stream_id: StreamId, user_id: UserId) {
        if let Some(subscribers) = self.subscriptions.get_mut(&stream_id) {
            subscribers.remove(&user_id);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedStream {
    pub stream_id: StreamId,
    pub publisher_id: UserId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_deduplicates_streams() {
        let mut room = Room::new(1, "general");
        room.publish_stream(PublishedStream {
            stream_id: 7,
            publisher_id: 1,
        });
        room.publish_stream(PublishedStream {
            stream_id: 7,
            publisher_id: 1,
        });

        assert_eq!(room.published_streams.len(), 1);
    }

    #[test]
    fn subscriptions_require_published_stream() {
        let mut room = Room::new(1, "general");
        assert!(!room.subscribe(9, 2));

        room.publish_stream(PublishedStream {
            stream_id: 9,
            publisher_id: 1,
        });
        assert!(room.subscribe(9, 2));
    }
}
