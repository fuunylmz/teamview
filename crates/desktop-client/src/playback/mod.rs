use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::decode::{DecodedFrame, DecodedPixelFormat};

pub trait VideoPlayback {
    fn render(&mut self, frame: DecodedFrame) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedFrame {
    pub frame_id: u32,
    pub width: u32,
    pub height: u32,
    pub pixel_format: DecodedPixelFormat,
    pub pixel_bytes: usize,
    pub render_time_micros: u64,
}

#[derive(Debug, Default)]
pub struct LatestFramePlayback {
    rendered_frames: u64,
    latest: Option<RenderedFrame>,
}

impl LatestFramePlayback {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rendered_frames(&self) -> u64 {
        self.rendered_frames
    }

    pub fn latest(&self) -> Option<&RenderedFrame> {
        self.latest.as_ref()
    }
}

impl VideoPlayback for LatestFramePlayback {
    fn render(&mut self, frame: DecodedFrame) -> anyhow::Result<()> {
        let expected_bytes = frame
            .width
            .checked_mul(frame.height)
            .and_then(|pixels| pixels.checked_mul(4))
            .map(|bytes| bytes as usize)
            .ok_or_else(|| anyhow::anyhow!("decoded frame dimensions overflow"))?;
        if frame.pixel_format != DecodedPixelFormat::Bgra8 {
            anyhow::bail!("unsupported decoded pixel format");
        }
        if frame.pixels.len() != expected_bytes {
            anyhow::bail!(
                "decoded frame pixel buffer length mismatch: expected {}, got {}",
                expected_bytes,
                frame.pixels.len()
            );
        }

        self.rendered_frames = self.rendered_frames.saturating_add(1);
        self.latest = Some(RenderedFrame {
            frame_id: frame.frame_id,
            width: frame.width,
            height: frame.height,
            pixel_format: frame.pixel_format,
            pixel_bytes: frame.pixels.len(),
            render_time_micros: unix_time_micros(),
        });
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct NullPlayback;

impl VideoPlayback for NullPlayback {
    fn render(&mut self, _frame: DecodedFrame) -> anyhow::Result<()> {
        Ok(())
    }
}

fn unix_time_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    #[test]
    fn latest_playback_keeps_rendered_frame_summary() {
        let mut playback = LatestFramePlayback::new();

        playback
            .render(DecodedFrame {
                frame_id: 7,
                width: 2,
                height: 1,
                pixel_format: DecodedPixelFormat::Bgra8,
                pixels: Bytes::from_static(&[0, 0, 0, 255, 1, 1, 1, 255]),
            })
            .unwrap();

        assert_eq!(playback.rendered_frames(), 1);
        let latest = playback.latest().unwrap();
        assert_eq!(latest.frame_id, 7);
        assert_eq!(latest.pixel_bytes, 8);
        assert!(latest.render_time_micros > 0);
    }

    #[test]
    fn latest_playback_rejects_bad_pixel_buffer_length() {
        let mut playback = LatestFramePlayback::new();

        let result = playback.render(DecodedFrame {
            frame_id: 7,
            width: 2,
            height: 1,
            pixel_format: DecodedPixelFormat::Bgra8,
            pixels: Bytes::from_static(&[0, 0, 0, 255]),
        });

        assert!(result.is_err());
    }
}
