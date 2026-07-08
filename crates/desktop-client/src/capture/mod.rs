pub mod windows;

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureFrame {
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub capture_time_micros: u64,
    pub format: CapturePixelFormat,
    pub storage: CaptureFrameStorage,
}

impl CaptureFrame {
    pub fn metadata_only(frame_id: u64, width: u32, height: u32, capture_time_micros: u64) -> Self {
        Self {
            frame_id,
            width,
            height,
            capture_time_micros,
            format: CapturePixelFormat::Bgra8,
            storage: CaptureFrameStorage::MetadataOnly,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePixelFormat {
    Bgra8,
    Nv12,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureFrameStorage {
    MetadataOnly,
    CpuBytes(Vec<u8>),
    NativeHandle(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureSource {
    PrimaryMonitor,
    Monitor { id: String },
    Window { id: String, title: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureConfig {
    pub queue_capacity: usize,
    pub cursor_visible: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 1,
            cursor_visible: true,
        }
    }
}

pub trait ScreenCapture {
    fn next_frame(&mut self) -> anyhow::Result<Option<CaptureFrame>>;
}

#[derive(Debug, Clone)]
pub struct LatestFrameQueue {
    frames: VecDeque<CaptureFrame>,
    capacity: usize,
    dropped_frames: u64,
}

impl LatestFrameQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            frames: VecDeque::new(),
            capacity: capacity.max(1),
            dropped_frames: 0,
        }
    }

    pub fn push(&mut self, frame: CaptureFrame) {
        while self.frames.len() >= self.capacity {
            self.frames.pop_front();
            self.dropped_frames = self.dropped_frames.saturating_add(1);
        }
        self.frames.push_back(frame);
    }

    pub fn pop_latest(&mut self) -> Option<CaptureFrame> {
        let latest = self.frames.pop_back()?;
        self.frames.clear();
        Some(latest)
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_frame_queue_keeps_only_latest_frame_by_default() {
        let mut queue = LatestFrameQueue::new(1);
        queue.push(CaptureFrame::metadata_only(1, 1280, 720, 10));
        queue.push(CaptureFrame::metadata_only(2, 1280, 720, 20));
        queue.push(CaptureFrame::metadata_only(3, 1280, 720, 30));

        assert_eq!(queue.len(), 1);
        assert_eq!(queue.dropped_frames(), 2);
        assert_eq!(queue.pop_latest().unwrap().frame_id, 3);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn latest_frame_queue_capacity_is_never_zero() {
        let mut queue = LatestFrameQueue::new(0);
        queue.push(CaptureFrame::metadata_only(1, 1, 1, 1));
        queue.push(CaptureFrame::metadata_only(2, 1, 1, 2));

        assert_eq!(queue.len(), 1);
        assert_eq!(queue.pop_latest().unwrap().frame_id, 2);
    }
}
