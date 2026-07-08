use std::collections::{BTreeMap, BTreeSet, VecDeque};

use teamview_protocol::{
    control::{StreamId, UserId},
    packet::MediaPacket,
    stats::QueueStats,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueDecision {
    Enqueue,
    DropForViewer,
}

pub fn decide_enqueue(queue: QueueStats, queue_budget_ms: u16) -> EnqueueDecision {
    if queue.is_over_budget(queue_budget_ms) {
        EnqueueDecision::DropForViewer
    } else {
        EnqueueDecision::Enqueue
    }
}

#[derive(Debug, Clone)]
pub struct ViewerQueue {
    viewer_id: UserId,
    queued_packets: VecDeque<QueuedPacket>,
    queued_media_ms: u16,
    queue_budget_ms: u16,
    received_packets: u64,
    dropped_packets: u64,
    dropped_frames: u64,
}

impl ViewerQueue {
    pub fn new(viewer_id: UserId, queue_budget_ms: u16) -> Self {
        Self {
            viewer_id,
            queued_packets: VecDeque::new(),
            queued_media_ms: 0,
            queue_budget_ms,
            received_packets: 0,
            dropped_packets: 0,
            dropped_frames: 0,
        }
    }

    pub fn viewer_id(&self) -> UserId {
        self.viewer_id
    }

    pub fn enqueue(&mut self, packet: MediaPacket, media_duration_ms: u16) -> EnqueueDecision {
        if self.queued_media_ms.saturating_add(media_duration_ms) > self.queue_budget_ms {
            self.dropped_packets = self.dropped_packets.saturating_add(1);
            if packet
                .header
                .flags
                .contains(teamview_protocol::packet::PacketFlags::END_OF_FRAME)
            {
                self.dropped_frames = self.dropped_frames.saturating_add(1);
            }
            return EnqueueDecision::DropForViewer;
        }

        self.received_packets = self.received_packets.saturating_add(1);
        self.queued_media_ms = self.queued_media_ms.saturating_add(media_duration_ms);
        self.queued_packets.push_back(QueuedPacket {
            packet,
            media_duration_ms,
        });
        EnqueueDecision::Enqueue
    }

    pub fn drain_one(&mut self) -> Option<MediaPacket> {
        let queued = self.queued_packets.pop_front()?;
        self.queued_media_ms = self
            .queued_media_ms
            .saturating_sub(queued.media_duration_ms);
        Some(queued.packet)
    }

    pub fn stats(&self) -> QueueStats {
        QueueStats {
            queued_packets: self.queued_packets.len() as u32,
            queued_media_ms: self.queued_media_ms,
            dropped_packets: self.dropped_packets,
            dropped_frames: self.dropped_frames,
        }
    }

    pub fn received_packets(&self) -> u64 {
        self.received_packets
    }
}

#[derive(Debug, Clone)]
struct QueuedPacket {
    packet: MediaPacket,
    media_duration_ms: u16,
}

#[derive(Debug, Clone)]
pub struct StreamFanout {
    stream_id: StreamId,
    viewers: BTreeMap<UserId, ViewerQueue>,
}

impl StreamFanout {
    pub fn new(stream_id: StreamId) -> Self {
        Self {
            stream_id,
            viewers: BTreeMap::new(),
        }
    }

    pub fn add_viewer(&mut self, viewer_id: UserId, queue_budget_ms: u16) {
        self.viewers
            .entry(viewer_id)
            .or_insert_with(|| ViewerQueue::new(viewer_id, queue_budget_ms));
    }

    pub fn remove_viewer(&mut self, viewer_id: UserId) {
        self.viewers.remove(&viewer_id);
    }

    pub fn fanout(&mut self, packet: MediaPacket, media_duration_ms: u16) -> FanoutSummary {
        let mut delivered_to = BTreeSet::new();
        let mut dropped_for = BTreeSet::new();

        for (viewer_id, queue) in &mut self.viewers {
            match queue.enqueue(packet.clone(), media_duration_ms) {
                EnqueueDecision::Enqueue => {
                    delivered_to.insert(*viewer_id);
                }
                EnqueueDecision::DropForViewer => {
                    dropped_for.insert(*viewer_id);
                }
            }
        }

        FanoutSummary {
            stream_id: self.stream_id,
            delivered_to,
            dropped_for,
        }
    }

    pub fn viewer_queue(&self, viewer_id: UserId) -> Option<&ViewerQueue> {
        self.viewers.get(&viewer_id)
    }

    pub fn viewer_queue_mut(&mut self, viewer_id: UserId) -> Option<&mut ViewerQueue> {
        self.viewers.get_mut(&viewer_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanoutSummary {
    pub stream_id: StreamId,
    pub delivered_to: BTreeSet<UserId>,
    pub dropped_for: BTreeSet<UserId>,
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use teamview_protocol::{
        codec::CodecId,
        packet::{MediaPacketHeader, PacketFlags, PacketType},
    };

    use super::*;

    #[test]
    fn slow_viewer_queue_drops_without_blocking_router() {
        let queue = QueueStats {
            queued_packets: 30,
            queued_media_ms: 120,
            dropped_packets: 0,
            dropped_frames: 0,
        };

        assert_eq!(decide_enqueue(queue, 100), EnqueueDecision::DropForViewer);
    }

    #[test]
    fn slow_viewer_does_not_block_fast_viewer() {
        let mut fanout = StreamFanout::new(9);
        fanout.add_viewer(1, 100);
        fanout.add_viewer(2, 20);

        let packet = synthetic_packet(9, 1, 1);
        assert_eq!(fanout.fanout(packet.clone(), 20).delivered_to.len(), 2);

        let summary = fanout.fanout(packet, 20);
        assert!(summary.delivered_to.contains(&1));
        assert!(summary.dropped_for.contains(&2));
        assert_eq!(fanout.viewer_queue(1).unwrap().received_packets(), 2);
        assert_eq!(fanout.viewer_queue(2).unwrap().received_packets(), 1);
        assert_eq!(fanout.viewer_queue(2).unwrap().stats().dropped_packets, 1);
    }

    #[test]
    fn draining_viewer_queue_releases_only_that_viewer_budget() {
        let mut fanout = StreamFanout::new(9);
        fanout.add_viewer(1, 20);
        fanout.add_viewer(2, 20);

        let packet = synthetic_packet(9, 1, 1);
        fanout.fanout(packet.clone(), 20);
        fanout.viewer_queue_mut(1).unwrap().drain_one();
        let summary = fanout.fanout(packet, 20);

        assert!(summary.delivered_to.contains(&1));
        assert!(summary.dropped_for.contains(&2));
    }

    fn synthetic_packet(stream_id: StreamId, sequence_number: u32, frame_id: u32) -> MediaPacket {
        let payload = Bytes::from_static(b"synthetic");
        let mut header = MediaPacketHeader::new(
            PacketType::Video,
            CodecId::H264,
            stream_id,
            sequence_number,
            payload.len() as u16,
        );
        header.frame_id = frame_id;
        header.flags = PacketFlags::empty().with(PacketFlags::END_OF_FRAME);
        MediaPacket { header, payload }
    }
}
