use teamview_protocol::stats::QueueStats;

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

#[cfg(test)]
mod tests {
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
}
