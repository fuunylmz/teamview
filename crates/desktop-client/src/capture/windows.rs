use super::{CaptureFrame, ScreenCapture};

#[derive(Debug, Default)]
pub struct WindowsGraphicsCapture;

impl ScreenCapture for WindowsGraphicsCapture {
    fn next_frame(&mut self) -> anyhow::Result<Option<CaptureFrame>> {
        Ok(None)
    }
}
