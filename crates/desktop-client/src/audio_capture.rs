use std::collections::VecDeque;

#[cfg(target_os = "windows")]
use std::{mem, ptr};

#[cfg(target_os = "windows")]
use windows_sys::Win32::Media::Audio::{WAVEINCAPSW, waveInGetDevCapsW, waveInGetNumDevs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicrophoneSource {
    Default,
    Device { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicrophoneSourceInfo {
    pub source: MicrophoneSource,
    pub label: String,
    pub sample_rate_hz: u32,
    pub channel_count: u16,
    pub is_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioCaptureConfig {
    pub sample_rate_hz: u32,
    pub channel_count: u16,
    pub frame_duration_ms: u16,
    pub queue_capacity: usize,
}

impl Default for AudioCaptureConfig {
    fn default() -> Self {
        Self {
            sample_rate_hz: 48_000,
            channel_count: 1,
            frame_duration_ms: 20,
            queue_capacity: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedAudioFrame {
    pub frame_id: u64,
    pub capture_time_micros: u64,
    pub sample_rate_hz: u32,
    pub channel_count: u16,
    pub samples: Vec<i16>,
}

impl CapturedAudioFrame {
    pub fn pcm_i16(
        frame_id: u64,
        capture_time_micros: u64,
        sample_rate_hz: u32,
        channel_count: u16,
        samples: Vec<i16>,
    ) -> anyhow::Result<Self> {
        if sample_rate_hz == 0 {
            anyhow::bail!("audio sample rate must be non-zero");
        }
        if channel_count == 0 {
            anyhow::bail!("audio channel count must be non-zero");
        }
        if !samples.len().is_multiple_of(channel_count as usize) {
            anyhow::bail!(
                "audio sample buffer length {} is not divisible by channel count {}",
                samples.len(),
                channel_count
            );
        }
        Ok(Self {
            frame_id,
            capture_time_micros,
            sample_rate_hz,
            channel_count,
            samples,
        })
    }

    pub fn sample_count_per_channel(&self) -> u16 {
        self.samples
            .len()
            .saturating_div(self.channel_count.max(1) as usize)
            .min(u16::MAX as usize) as u16
    }
}

pub trait MicrophoneCapture {
    fn next_frame(&mut self) -> anyhow::Result<Option<CapturedAudioFrame>>;
}

#[derive(Debug, Clone)]
pub struct LatestAudioCaptureQueue {
    frames: VecDeque<CapturedAudioFrame>,
    capacity: usize,
    dropped_frames: u64,
}

impl LatestAudioCaptureQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            frames: VecDeque::new(),
            capacity: capacity.max(1),
            dropped_frames: 0,
        }
    }

    pub fn push(&mut self, frame: CapturedAudioFrame) {
        while self.frames.len() >= self.capacity {
            self.frames.pop_front();
            self.dropped_frames = self.dropped_frames.saturating_add(1);
        }
        self.frames.push_back(frame);
    }

    pub fn pop_latest(&mut self) -> Option<CapturedAudioFrame> {
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

#[derive(Debug)]
pub struct WindowsMicrophoneCapture {
    source: MicrophoneSource,
    config: AudioCaptureConfig,
    queue: LatestAudioCaptureQueue,
    next_frame_id: u64,
}

impl WindowsMicrophoneCapture {
    pub fn new(source: MicrophoneSource, config: AudioCaptureConfig) -> anyhow::Result<Self> {
        ensure_supported()?;
        Ok(Self {
            source,
            config,
            queue: LatestAudioCaptureQueue::new(config.queue_capacity),
            next_frame_id: 1,
        })
    }

    pub fn source(&self) -> &MicrophoneSource {
        &self.source
    }

    pub fn config(&self) -> AudioCaptureConfig {
        self.config
    }

    pub fn queue_dropped_frames(&self) -> u64 {
        self.queue.dropped_frames()
    }

    pub fn push_silence_frame(&mut self, capture_time_micros: u64) -> anyhow::Result<()> {
        let sample_count = self
            .config
            .sample_rate_hz
            .saturating_mul(self.config.frame_duration_ms as u32)
            .saturating_div(1_000)
            .max(1) as usize;
        let total_samples = sample_count.saturating_mul(self.config.channel_count.max(1) as usize);
        let frame = CapturedAudioFrame::pcm_i16(
            self.next_frame_id,
            capture_time_micros,
            self.config.sample_rate_hz,
            self.config.channel_count,
            vec![0; total_samples],
        )?;
        self.next_frame_id = self.next_frame_id.saturating_add(1);
        self.queue.push(frame);
        Ok(())
    }
}

impl MicrophoneCapture for WindowsMicrophoneCapture {
    fn next_frame(&mut self) -> anyhow::Result<Option<CapturedAudioFrame>> {
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
        anyhow::bail!("microphone capture is only available on Windows")
    }
}

pub fn list_microphone_sources() -> anyhow::Result<Vec<MicrophoneSourceInfo>> {
    ensure_supported()?;
    list_microphone_sources_impl()
}

#[cfg(target_os = "windows")]
fn list_microphone_sources_impl() -> anyhow::Result<Vec<MicrophoneSourceInfo>> {
    let device_count = unsafe { waveInGetNumDevs() };
    let mut sources = Vec::with_capacity(device_count as usize);
    for device_index in 0..device_count {
        let mut caps = unsafe { mem::zeroed::<WAVEINCAPSW>() };
        let result = unsafe {
            waveInGetDevCapsW(
                device_index as usize,
                &mut caps,
                mem::size_of::<WAVEINCAPSW>() as u32,
            )
        };
        if result != 0 {
            continue;
        }
        let device_name = unsafe { ptr::addr_of!(caps.szPname).read_unaligned() };
        sources.push(MicrophoneSourceInfo {
            source: MicrophoneSource::Device {
                id: device_index.to_string(),
            },
            label: wide_device_name(&device_name),
            sample_rate_hz: AudioCaptureConfig::default().sample_rate_hz,
            channel_count: caps.wChannels.max(1),
            is_default: device_index == 0,
        });
    }
    Ok(sources)
}

#[cfg(not(target_os = "windows"))]
fn list_microphone_sources_impl() -> anyhow::Result<Vec<MicrophoneSourceInfo>> {
    anyhow::bail!("microphone capture is only available on Windows")
}

#[cfg(target_os = "windows")]
fn wide_device_name(name: &[u16]) -> String {
    let end = name
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(name.len());
    String::from_utf16_lossy(&name[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_detection_matches_target_os() {
        assert_eq!(is_supported(), cfg!(target_os = "windows"));
    }

    #[test]
    fn latest_audio_capture_queue_keeps_only_latest_frame() {
        let mut queue = LatestAudioCaptureQueue::new(1);
        queue.push(CapturedAudioFrame::pcm_i16(1, 10, 48_000, 1, vec![1, 2]).unwrap());
        queue.push(CapturedAudioFrame::pcm_i16(2, 20, 48_000, 1, vec![3, 4]).unwrap());

        assert_eq!(queue.len(), 1);
        assert_eq!(queue.dropped_frames(), 1);
        assert_eq!(queue.pop_latest().unwrap().frame_id, 2);
    }

    #[test]
    fn captured_audio_frame_validates_channel_layout() {
        let frame = CapturedAudioFrame::pcm_i16(1, 10, 48_000, 2, vec![1, 2, 3, 4]).unwrap();
        assert_eq!(frame.sample_count_per_channel(), 2);

        let error = CapturedAudioFrame::pcm_i16(1, 10, 48_000, 2, vec![1, 2, 3]).unwrap_err();
        assert!(error.to_string().contains("not divisible"));
    }

    #[test]
    fn microphone_capture_uses_test_silence_queue() {
        if !is_supported() {
            return;
        }
        let mut capture =
            WindowsMicrophoneCapture::new(MicrophoneSource::Default, AudioCaptureConfig::default())
                .unwrap();

        capture.push_silence_frame(123).unwrap();
        let frame = capture.next_frame().unwrap().unwrap();

        assert_eq!(frame.frame_id, 1);
        assert_eq!(frame.capture_time_micros, 123);
        assert_eq!(frame.sample_rate_hz, 48_000);
        assert_eq!(frame.channel_count, 1);
        assert_eq!(frame.sample_count_per_channel(), 960);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn wide_device_name_trims_at_nul() {
        let mut name = [0_u16; 32];
        name[0] = 'M' as u16;
        name[1] = 'i' as u16;
        name[2] = 'c' as u16;

        assert_eq!(wide_device_name(&name), "Mic");
    }
}
