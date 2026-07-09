use std::collections::{BTreeMap, BTreeSet};

use teamview_protocol::{
    codec::CodecId,
    control::{MediaKind, RoomId, StreamConfig, StreamId, UserId},
};

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
        self.subscriptions
            .retain(|_, subscribers| !subscribers.is_empty());
    }

    pub fn publish_stream(&mut self, stream: PublishedStream) {
        self.published_streams.insert(stream.stream_id, stream);
    }

    pub fn streams_published_by(&self, user_id: UserId) -> Vec<StreamId> {
        self.published_streams
            .iter()
            .filter_map(|(stream_id, stream)| {
                (stream.publisher_id == user_id).then_some(*stream_id)
            })
            .collect()
    }

    pub fn remove_published_stream(&mut self, stream_id: StreamId) -> Option<PublishedStream> {
        let removed = self.published_streams.remove(&stream_id);
        self.subscriptions.remove(&stream_id);
        removed
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
        let should_remove = if let Some(subscribers) = self.subscriptions.get_mut(&stream_id) {
            subscribers.remove(&user_id);
            subscribers.is_empty()
        } else {
            false
        };
        if should_remove {
            self.subscriptions.remove(&stream_id);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.participants.is_empty() && self.published_streams.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedStream {
    pub stream_id: StreamId,
    pub publisher_id: UserId,
    pub codec: CodecId,
    pub media_kind: MediaKind,
    pub config: Option<StreamConfig>,
    pub target_bitrate_bps: u32,
    pub target_frames_per_second: u16,
    pub target_width: u32,
    pub target_height: u32,
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
            codec: CodecId::H264,
            media_kind: MediaKind::Screen,
            config: None,
            target_bitrate_bps: 4_000_000,
            target_frames_per_second: 30,
            target_width: 1280,
            target_height: 720,
        });
        room.publish_stream(PublishedStream {
            stream_id: 7,
            publisher_id: 1,
            codec: CodecId::H264,
            media_kind: MediaKind::Screen,
            config: None,
            target_bitrate_bps: 4_000_000,
            target_frames_per_second: 30,
            target_width: 1280,
            target_height: 720,
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
            codec: CodecId::H264,
            media_kind: MediaKind::Screen,
            config: None,
            target_bitrate_bps: 4_000_000,
            target_frames_per_second: 30,
            target_width: 1280,
            target_height: 720,
        });
        assert!(room.subscribe(9, 2));
    }

    #[test]
    fn leave_and_unsubscribe_prune_empty_subscription_sets() {
        let mut room = Room::new(1, "general");
        room.join(2);
        room.publish_stream(PublishedStream {
            stream_id: 9,
            publisher_id: 1,
            codec: CodecId::H264,
            media_kind: MediaKind::Screen,
            config: None,
            target_bitrate_bps: 4_000_000,
            target_frames_per_second: 30,
            target_width: 1280,
            target_height: 720,
        });
        assert!(room.subscribe(9, 2));

        room.unsubscribe(9, 2);

        assert!(!room.subscriptions.contains_key(&9));

        assert!(room.subscribe(9, 2));
        room.leave(2);

        assert!(!room.subscriptions.contains_key(&9));
    }

    #[test]
    fn room_reports_empty_after_last_participant_and_stream_leave() {
        let mut room = Room::new(1, "general");
        room.join(1);
        room.publish_stream(PublishedStream {
            stream_id: 9,
            publisher_id: 1,
            codec: CodecId::H264,
            media_kind: MediaKind::Screen,
            config: None,
            target_bitrate_bps: 4_000_000,
            target_frames_per_second: 30,
            target_width: 1280,
            target_height: 720,
        });

        assert!(!room.is_empty());
        room.leave(1);
        room.remove_published_stream(9);

        assert!(room.is_empty());
    }
}
