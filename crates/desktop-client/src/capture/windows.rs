use super::{CaptureConfig, CaptureFrame, CaptureSource, LatestFrameQueue, ScreenCapture};

#[derive(Debug)]
pub struct WindowsGraphicsCapture {
    source: CaptureSource,
    config: CaptureConfig,
    queue: LatestFrameQueue,
    next_frame_id: u64,
}

impl WindowsGraphicsCapture {
    pub fn new(source: CaptureSource, config: CaptureConfig) -> anyhow::Result<Self> {
        ensure_supported()?;
        Ok(Self {
            source,
            config,
            queue: LatestFrameQueue::new(config.queue_capacity),
            next_frame_id: 1,
        })
    }

    pub fn source(&self) -> &CaptureSource {
        &self.source
    }

    pub fn config(&self) -> CaptureConfig {
        self.config
    }

    pub fn queue_dropped_frames(&self) -> u64 {
        self.queue.dropped_frames()
    }

    pub fn push_test_frame(&mut self, width: u32, height: u32, capture_time_micros: u64) {
        let frame =
            CaptureFrame::metadata_only(self.next_frame_id, width, height, capture_time_micros);
        self.next_frame_id = self.next_frame_id.saturating_add(1);
        self.queue.push(frame);
    }
}

impl ScreenCapture for WindowsGraphicsCapture {
    fn next_frame(&mut self) -> anyhow::Result<Option<CaptureFrame>> {
        Ok(self.queue.pop_latest())
    }
}

pub fn is_supported() -> bool {
    cfg!(target_os = "windows")
}

pub fn ensure_supported() -> anyhow::Result<()> {
    if is_supported() {
        Ok(())
    } else {
        anyhow::bail!("Windows Graphics Capture is only available on Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_detection_matches_target_os() {
        assert_eq!(is_supported(), cfg!(target_os = "windows"));
    }

    #[test]
    fn capture_returns_latest_queued_frame() {
        let mut capture = WindowsGraphicsCapture::new(
            CaptureSource::PrimaryMonitor,
            CaptureConfig {
                queue_capacity: 1,
                cursor_visible: true,
            },
        )
        .unwrap();

        capture.push_test_frame(1280, 720, 10);
        capture.push_test_frame(1280, 720, 20);

        let frame = capture.next_frame().unwrap().unwrap();
        assert_eq!(frame.frame_id, 2);
        assert_eq!(frame.capture_time_micros, 20);
        assert_eq!(capture.queue_dropped_frames(), 1);
    }
}
