pub mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureFrame {
    pub width: u32,
    pub height: u32,
    pub capture_time_micros: u64,
}

pub trait ScreenCapture {
    fn next_frame(&mut self) -> anyhow::Result<Option<CaptureFrame>>;
}
