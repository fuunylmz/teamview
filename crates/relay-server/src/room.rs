use teamview_protocol::control::{RoomId, StreamId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub id: RoomId,
    pub name: String,
    pub published_streams: Vec<StreamId>,
}

impl Room {
    pub fn new(id: RoomId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            published_streams: Vec::new(),
        }
    }

    pub fn publish_stream(&mut self, stream_id: StreamId) {
        if !self.published_streams.contains(&stream_id) {
            self.published_streams.push(stream_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_deduplicates_streams() {
        let mut room = Room::new(1, "general");
        room.publish_stream(7);
        room.publish_stream(7);

        assert_eq!(room.published_streams, vec![7]);
    }
}
