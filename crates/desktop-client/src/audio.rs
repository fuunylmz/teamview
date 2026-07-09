use bytes::Bytes;
use teamview_protocol::{codec::CodecId, frame::EncodedFrame};

#[cfg(target_os = "windows")]
use std::{collections::VecDeque, fmt, mem, ptr};

#[cfg(target_os = "windows")]
use windows_sys::Win32::Media::Audio::{
    CALLBACK_NULL, HWAVEOUT, WAVE_FORMAT_PCM, WAVE_MAPPER, WAVEFORMATEX, WAVEHDR, WHDR_DONE,
    waveOutClose, waveOutOpen, waveOutPrepareHeader, waveOutReset, waveOutUnprepareHeader,
    waveOutWrite,
};

const SYNTHETIC_OPUS_MAGIC: &[u8; 4] = b"TVO1";
const PCM_PAYLOAD_MAGIC: &[u8; 4] = b"TVP1";
const MAX_PENDING_SPEAKER_BUFFERS: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticOpusEncoderConfig {
    pub sample_rate_hz: u32,
    pub channel_count: u16,
    pub frame_duration_ms: u16,
    pub bitrate_bps: u32,
    pub synthetic_payload_bytes: usize,
}

impl Default for SyntheticOpusEncoderConfig {
    fn default() -> Self {
        Self {
            sample_rate_hz: 48_000,
            channel_count: 1,
            frame_duration_ms: 20,
            bitrate_bps: 32_000,
            synthetic_payload_bytes: 96,
        }
    }
}

#[derive(Debug, Default)]
pub struct SyntheticOpusEncoder {
    pub config: SyntheticOpusEncoderConfig,
}

impl SyntheticOpusEncoder {
    pub fn encode(
        &self,
        frame_id: u32,
        capture_time_micros: u64,
        stream_id: u32,
    ) -> anyhow::Result<EncodedFrame> {
        let sample_count = self.sample_count_per_frame();
        let mut bytes = Vec::with_capacity(self.config.synthetic_payload_bytes.max(20));
        bytes.extend_from_slice(SYNTHETIC_OPUS_MAGIC);
        bytes.extend_from_slice(&frame_id.to_le_bytes());
        bytes.extend_from_slice(&self.config.sample_rate_hz.to_le_bytes());
        bytes.extend_from_slice(&self.config.channel_count.to_le_bytes());
        bytes.extend_from_slice(&sample_count.to_le_bytes());
        bytes.extend_from_slice(&capture_time_micros.to_le_bytes());
        while bytes.len() < self.config.synthetic_payload_bytes {
            bytes.push(
                frame_id
                    .wrapping_mul(31)
                    .wrapping_add(bytes.len() as u32)
                    .to_le_bytes()[0],
            );
        }

        Ok(EncodedFrame {
            room_stream_id: stream_id,
            frame_id,
            media_timestamp: frame_id.saturating_mul(sample_count as u32) as u64,
            sender_capture_time_micros: capture_time_micros,
            sender_clock_offset_micros: 0,
            sender_encode_done_time_micros: 0,
            sender_send_time_micros: 0,
            server_receive_time_micros: 0,
            server_send_time_micros: 0,
            codec: CodecId::Opus,
            is_keyframe: false,
            bytes: Bytes::from(bytes),
        })
    }

    pub fn encode_pcm_i16(
        &self,
        frame_id: u32,
        capture_time_micros: u64,
        sample_rate_hz: u32,
        channel_count: u16,
        samples: &[i16],
        stream_id: u32,
    ) -> anyhow::Result<EncodedFrame> {
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
        let sample_count_per_channel = samples.len().saturating_div(channel_count as usize);
        if sample_count_per_channel == 0 {
            anyhow::bail!("audio sample count must be non-zero");
        }
        if sample_count_per_channel > u16::MAX as usize {
            anyhow::bail!("audio sample count exceeds protocol field");
        }
        let sample_count = sample_count_per_channel as u16;

        let pcm_bytes = samples
            .len()
            .checked_mul(std::mem::size_of::<i16>())
            .ok_or_else(|| anyhow::anyhow!("audio PCM payload size overflow"))?;
        if pcm_bytes > u32::MAX as usize {
            anyhow::bail!("audio PCM payload exceeds protocol length field");
        }
        let mut bytes =
            Vec::with_capacity(self.config.synthetic_payload_bytes.max(24 + 8 + pcm_bytes));
        bytes.extend_from_slice(SYNTHETIC_OPUS_MAGIC);
        bytes.extend_from_slice(&frame_id.to_le_bytes());
        bytes.extend_from_slice(&sample_rate_hz.to_le_bytes());
        bytes.extend_from_slice(&channel_count.to_le_bytes());
        bytes.extend_from_slice(&sample_count.to_le_bytes());
        bytes.extend_from_slice(&capture_time_micros.to_le_bytes());
        bytes.extend_from_slice(PCM_PAYLOAD_MAGIC);
        bytes.extend_from_slice(&(pcm_bytes as u32).to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        while bytes.len() < self.config.synthetic_payload_bytes {
            bytes.push(
                frame_id
                    .wrapping_mul(17)
                    .wrapping_add(bytes.len() as u32)
                    .to_le_bytes()[0],
            );
        }

        Ok(EncodedFrame {
            room_stream_id: stream_id,
            frame_id,
            media_timestamp: frame_id.saturating_mul(sample_count as u32) as u64,
            sender_capture_time_micros: capture_time_micros,
            sender_clock_offset_micros: 0,
            sender_encode_done_time_micros: 0,
            sender_send_time_micros: 0,
            server_receive_time_micros: 0,
            server_send_time_micros: 0,
            codec: CodecId::Opus,
            is_keyframe: false,
            bytes: Bytes::from(bytes),
        })
    }

    pub fn update_bitrate(&mut self, bitrate_bps: u32) {
        self.config.bitrate_bps = bitrate_bps;
        self.config.synthetic_payload_bytes =
            synthetic_audio_payload_bytes(bitrate_bps, self.frames_per_second());
    }

    pub fn set_frames_per_second(&mut self, frames_per_second: u16) {
        self.config.frame_duration_ms = frame_duration_ms(frames_per_second);
        self.config.synthetic_payload_bytes =
            synthetic_audio_payload_bytes(self.config.bitrate_bps, self.frames_per_second());
    }

    pub fn frames_per_second(&self) -> u16 {
        (1_000 / self.config.frame_duration_ms.max(1)).max(1)
    }

    fn sample_count_per_frame(&self) -> u16 {
        self.config
            .sample_rate_hz
            .saturating_mul(self.config.frame_duration_ms as u32)
            .saturating_div(1_000)
            .min(u16::MAX as u32) as u16
    }
}

#[derive(Debug, Default)]
pub struct SyntheticOpusDecoder;

impl SyntheticOpusDecoder {
    pub fn decode(&mut self, encoded: &[u8]) -> anyhow::Result<Option<DecodedAudioFrame>> {
        let Some(header) = encoded.get(..24) else {
            return Ok(None);
        };
        if &header[..4] != SYNTHETIC_OPUS_MAGIC {
            return Ok(None);
        }
        let frame_id = u32::from_le_bytes(header[4..8].try_into()?);
        let sample_rate_hz = u32::from_le_bytes(header[8..12].try_into()?);
        let channel_count = u16::from_le_bytes(header[12..14].try_into()?);
        let sample_count = u16::from_le_bytes(header[14..16].try_into()?);
        let capture_time_micros = u64::from_le_bytes(header[16..24].try_into()?);
        if sample_rate_hz == 0 || channel_count == 0 || sample_count == 0 {
            return Ok(None);
        }
        Ok(Some(DecodedAudioFrame {
            frame_id,
            sample_rate_hz,
            channel_count,
            sample_count,
            capture_time_micros,
            pcm: embedded_pcm(&encoded[24..], sample_count, channel_count)?
                .unwrap_or_else(|| synthetic_pcm(frame_id, sample_count, channel_count)),
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedAudioFrame {
    pub frame_id: u32,
    pub sample_rate_hz: u32,
    pub channel_count: u16,
    pub sample_count: u16,
    pub capture_time_micros: u64,
    pub pcm: Vec<i16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayedAudioFrame {
    pub frame_id: u32,
    pub sample_rate_hz: u32,
    pub channel_count: u16,
    pub sample_count: u16,
}

#[derive(Debug, Default)]
pub struct LatestAudioPlayback {
    played_frames: u64,
    latest: Option<PlayedAudioFrame>,
}

pub trait AudioPlayback {
    fn play(&mut self, frame: DecodedAudioFrame) -> anyhow::Result<()>;
}

#[derive(Debug)]
pub enum AudioOutputPlayback {
    Sink(LatestAudioPlayback),
    Speaker(SpeakerAudioPlayback),
}

impl AudioOutputPlayback {
    pub fn sink() -> Self {
        Self::Sink(LatestAudioPlayback::default())
    }

    pub fn speaker() -> anyhow::Result<Self> {
        Ok(Self::Speaker(SpeakerAudioPlayback::new()?))
    }

    pub fn played_frames(&self) -> u64 {
        match self {
            Self::Sink(playback) => playback.played_frames(),
            Self::Speaker(playback) => playback.played_frames(),
        }
    }

    pub fn latest(&self) -> Option<&PlayedAudioFrame> {
        match self {
            Self::Sink(playback) => playback.latest(),
            Self::Speaker(playback) => playback.latest(),
        }
    }
}

impl AudioPlayback for AudioOutputPlayback {
    fn play(&mut self, frame: DecodedAudioFrame) -> anyhow::Result<()> {
        match self {
            Self::Sink(playback) => playback.play(frame),
            Self::Speaker(playback) => playback.play(frame),
        }
    }
}

impl LatestAudioPlayback {
    pub fn play(&mut self, frame: DecodedAudioFrame) -> anyhow::Result<()> {
        let played = validate_decoded_audio_frame(&frame)?;
        self.played_frames = self.played_frames.saturating_add(1);
        self.latest = Some(played);
        Ok(())
    }

    pub fn played_frames(&self) -> u64 {
        self.played_frames
    }

    pub fn latest(&self) -> Option<&PlayedAudioFrame> {
        self.latest.as_ref()
    }
}

impl AudioPlayback for LatestAudioPlayback {
    fn play(&mut self, frame: DecodedAudioFrame) -> anyhow::Result<()> {
        Self::play(self, frame)
    }
}

#[derive(Debug)]
pub struct SpeakerAudioPlayback {
    latest: LatestAudioPlayback,
    #[cfg(target_os = "windows")]
    device: Win32WaveOutPlayback,
}

impl SpeakerAudioPlayback {
    pub fn new() -> anyhow::Result<Self> {
        ensure_speaker_supported()?;
        Ok(Self {
            latest: LatestAudioPlayback::default(),
            #[cfg(target_os = "windows")]
            device: Win32WaveOutPlayback::new(),
        })
    }

    pub fn played_frames(&self) -> u64 {
        self.latest.played_frames()
    }

    pub fn latest(&self) -> Option<&PlayedAudioFrame> {
        self.latest.latest()
    }
}

impl AudioPlayback for SpeakerAudioPlayback {
    fn play(&mut self, frame: DecodedAudioFrame) -> anyhow::Result<()> {
        let played = validate_decoded_audio_frame(&frame)?;
        #[cfg(target_os = "windows")]
        self.device.play(&frame)?;
        self.latest.played_frames = self.latest.played_frames.saturating_add(1);
        self.latest.latest = Some(played);
        Ok(())
    }
}

pub fn speaker_output_supported() -> bool {
    cfg!(target_os = "windows")
}

fn ensure_speaker_supported() -> anyhow::Result<()> {
    if speaker_output_supported() {
        Ok(())
    } else {
        anyhow::bail!("speaker audio output is only available on Windows")
    }
}

fn validate_decoded_audio_frame(frame: &DecodedAudioFrame) -> anyhow::Result<PlayedAudioFrame> {
    let expected_samples = frame
        .sample_count
        .checked_mul(frame.channel_count)
        .map(|samples| samples as usize)
        .ok_or_else(|| anyhow::anyhow!("decoded audio sample count overflow"))?;
    if frame.pcm.len() != expected_samples {
        anyhow::bail!(
            "decoded audio sample buffer length mismatch: expected {}, got {}",
            expected_samples,
            frame.pcm.len()
        );
    }
    Ok(PlayedAudioFrame {
        frame_id: frame.frame_id,
        sample_rate_hz: frame.sample_rate_hz,
        channel_count: frame.channel_count,
        sample_count: frame.sample_count,
    })
}

pub fn synthetic_audio_payload_bytes(bitrate_bps: u32, frames_per_second: u16) -> usize {
    bitrate_bps
        .saturating_div(8)
        .saturating_div(frames_per_second.max(1) as u32)
        .max(16) as usize
}

fn frame_duration_ms(frames_per_second: u16) -> u16 {
    (1_000 / frames_per_second.max(1)).max(1)
}

fn synthetic_pcm(frame_id: u32, sample_count: u16, channel_count: u16) -> Vec<i16> {
    let total_samples = sample_count as usize * channel_count as usize;
    (0..total_samples)
        .map(|sample| {
            let phase = frame_id as i32 * 97 + sample as i32 * 13;
            ((phase % 1024) - 512) as i16
        })
        .collect()
}

fn embedded_pcm(
    payload: &[u8],
    sample_count: u16,
    channel_count: u16,
) -> anyhow::Result<Option<Vec<i16>>> {
    let Some(header) = payload.get(..8) else {
        return Ok(None);
    };
    if &header[..4] != PCM_PAYLOAD_MAGIC {
        return Ok(None);
    }
    let pcm_byte_len = u32::from_le_bytes(header[4..8].try_into()?) as usize;
    let expected_pcm_bytes = sample_count as usize * channel_count as usize * 2;
    if pcm_byte_len != expected_pcm_bytes {
        anyhow::bail!(
            "embedded PCM length mismatch: expected {}, got {}",
            expected_pcm_bytes,
            pcm_byte_len
        );
    }
    let pcm_bytes = payload
        .get(8..8 + pcm_byte_len)
        .ok_or_else(|| anyhow::anyhow!("embedded PCM payload is truncated"))?;
    Ok(Some(
        pcm_bytes
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
            .collect(),
    ))
}

#[cfg(target_os = "windows")]
struct Win32WaveOutPlayback {
    handle: Option<HWAVEOUT>,
    sample_rate_hz: u32,
    channel_count: u16,
    pending: VecDeque<Box<WaveOutBuffer>>,
}

#[cfg(target_os = "windows")]
impl Win32WaveOutPlayback {
    fn new() -> Self {
        Self {
            handle: None,
            sample_rate_hz: 0,
            channel_count: 0,
            pending: VecDeque::new(),
        }
    }

    fn play(&mut self, frame: &DecodedAudioFrame) -> anyhow::Result<()> {
        self.ensure_open(frame.sample_rate_hz, frame.channel_count)?;
        self.reclaim_finished_buffers()?;
        if self.pending.len() >= MAX_PENDING_SPEAKER_BUFFERS {
            self.reset_pending_buffers()?;
        }

        let handle = self
            .handle
            .ok_or_else(|| anyhow::anyhow!("speaker output is not open"))?;
        let mut buffer = Box::new(WaveOutBuffer::from_pcm(&frame.pcm)?);
        mm_result("waveOutPrepareHeader", unsafe {
            waveOutPrepareHeader(handle, &mut buffer.header, wave_header_size())
        })?;
        buffer.prepared = true;
        let write_result = unsafe { waveOutWrite(handle, &mut buffer.header, wave_header_size()) };
        if write_result != 0 {
            let _ =
                unsafe { waveOutUnprepareHeader(handle, &mut buffer.header, wave_header_size()) };
            buffer.prepared = false;
            mm_result("waveOutWrite", write_result)?;
        }
        self.pending.push_back(buffer);
        Ok(())
    }

    fn ensure_open(&mut self, sample_rate_hz: u32, channel_count: u16) -> anyhow::Result<()> {
        if self.handle.is_some()
            && self.sample_rate_hz == sample_rate_hz
            && self.channel_count == channel_count
        {
            return Ok(());
        }

        self.close()?;
        let format = wave_format(sample_rate_hz, channel_count)?;
        let mut handle: HWAVEOUT = ptr::null_mut();
        mm_result("waveOutOpen", unsafe {
            waveOutOpen(&mut handle, WAVE_MAPPER, &format, 0, 0, CALLBACK_NULL)
        })?;
        self.handle = Some(handle);
        self.sample_rate_hz = sample_rate_hz;
        self.channel_count = channel_count;
        Ok(())
    }

    fn reclaim_finished_buffers(&mut self) -> anyhow::Result<()> {
        while self
            .pending
            .front()
            .is_some_and(|buffer| buffer.header.dwFlags & WHDR_DONE != 0)
        {
            let mut buffer = self.pending.pop_front().expect("front exists");
            self.unprepare_buffer(&mut buffer)?;
        }
        Ok(())
    }

    fn reset_pending_buffers(&mut self) -> anyhow::Result<()> {
        if let Some(handle) = self.handle {
            mm_result("waveOutReset", unsafe { waveOutReset(handle) })?;
        }
        while let Some(mut buffer) = self.pending.pop_front() {
            self.unprepare_buffer(&mut buffer)?;
        }
        Ok(())
    }

    fn unprepare_buffer(&self, buffer: &mut WaveOutBuffer) -> anyhow::Result<()> {
        if let (Some(handle), true) = (self.handle, buffer.prepared) {
            mm_result("waveOutUnprepareHeader", unsafe {
                waveOutUnprepareHeader(handle, &mut buffer.header, wave_header_size())
            })?;
            buffer.prepared = false;
        }
        Ok(())
    }

    fn close(&mut self) -> anyhow::Result<()> {
        if self.handle.is_none() {
            return Ok(());
        }
        self.reset_pending_buffers()?;
        if let Some(handle) = self.handle.take() {
            mm_result("waveOutClose", unsafe { waveOutClose(handle) })?;
        }
        self.sample_rate_hz = 0;
        self.channel_count = 0;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
impl Drop for Win32WaveOutPlayback {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(target_os = "windows")]
impl fmt::Debug for Win32WaveOutPlayback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Win32WaveOutPlayback")
            .field("handle", &self.handle)
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("channel_count", &self.channel_count)
            .field("pending", &self.pending.len())
            .finish()
    }
}

#[cfg(target_os = "windows")]
struct WaveOutBuffer {
    data: Vec<u8>,
    header: WAVEHDR,
    prepared: bool,
}

#[cfg(target_os = "windows")]
impl WaveOutBuffer {
    fn from_pcm(samples: &[i16]) -> anyhow::Result<Self> {
        let byte_len = samples
            .len()
            .checked_mul(mem::size_of::<i16>())
            .ok_or_else(|| anyhow::anyhow!("speaker output buffer size overflow"))?;
        if byte_len > u32::MAX as usize {
            anyhow::bail!("speaker output buffer exceeds WinMM length field");
        }

        let mut data = Vec::with_capacity(byte_len);
        for sample in samples {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let header = WAVEHDR {
            lpData: data.as_mut_ptr(),
            dwBufferLength: byte_len as u32,
            dwBytesRecorded: 0,
            dwUser: 0,
            dwFlags: 0,
            dwLoops: 0,
            lpNext: ptr::null_mut(),
            reserved: 0,
        };
        Ok(Self {
            data,
            header,
            prepared: false,
        })
    }
}

#[cfg(target_os = "windows")]
fn wave_format(sample_rate_hz: u32, channel_count: u16) -> anyhow::Result<WAVEFORMATEX> {
    if sample_rate_hz == 0 {
        anyhow::bail!("speaker output sample rate must be non-zero");
    }
    if channel_count == 0 {
        anyhow::bail!("speaker output channel count must be non-zero");
    }

    let bytes_per_sample = mem::size_of::<i16>() as u16;
    let block_align = channel_count
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| anyhow::anyhow!("speaker output block align overflow"))?;
    let avg_bytes_per_sec = sample_rate_hz
        .checked_mul(block_align as u32)
        .ok_or_else(|| anyhow::anyhow!("speaker output average bytes per second overflow"))?;

    Ok(WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: channel_count,
        nSamplesPerSec: sample_rate_hz,
        nAvgBytesPerSec: avg_bytes_per_sec,
        nBlockAlign: block_align,
        wBitsPerSample: bytes_per_sample * 8,
        cbSize: 0,
    })
}

#[cfg(target_os = "windows")]
fn wave_header_size() -> u32 {
    mem::size_of::<WAVEHDR>() as u32
}

#[cfg(target_os = "windows")]
fn mm_result(action: &str, result: u32) -> anyhow::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        anyhow::bail!("{action} failed with MMRESULT {}", result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_opus_round_trips_to_decoded_audio() {
        let encoder = SyntheticOpusEncoder::default();
        let encoded = encoder.encode(7, 123_456, 9).unwrap();
        let mut decoder = SyntheticOpusDecoder;

        let decoded = decoder.decode(&encoded.bytes).unwrap().unwrap();

        assert_eq!(encoded.codec, CodecId::Opus);
        assert_eq!(decoded.frame_id, 7);
        assert_eq!(decoded.sample_rate_hz, 48_000);
        assert_eq!(decoded.channel_count, 1);
        assert_eq!(decoded.sample_count, 960);
        assert_eq!(decoded.capture_time_micros, 123_456);
        assert_eq!(decoded.pcm.len(), 960);
    }

    #[test]
    fn synthetic_opus_embeds_pcm_payload_when_available() {
        let encoder = SyntheticOpusEncoder::default();
        let pcm = vec![-100, 0, 100, 200];
        let encoded = encoder.encode_pcm_i16(3, 42, 48_000, 2, &pcm, 7).unwrap();
        let mut decoder = SyntheticOpusDecoder;

        let decoded = decoder.decode(&encoded.bytes).unwrap().unwrap();

        assert_eq!(decoded.frame_id, 3);
        assert_eq!(decoded.channel_count, 2);
        assert_eq!(decoded.sample_count, 2);
        assert_eq!(decoded.capture_time_micros, 42);
        assert_eq!(decoded.pcm, pcm);
    }

    #[test]
    fn latest_audio_playback_keeps_latest_frame() {
        let mut playback = LatestAudioPlayback::default();

        playback
            .play(DecodedAudioFrame {
                frame_id: 1,
                sample_rate_hz: 48_000,
                channel_count: 2,
                sample_count: 2,
                capture_time_micros: 1,
                pcm: vec![0, 1, 2, 3],
            })
            .unwrap();

        assert_eq!(playback.played_frames(), 1);
        assert_eq!(playback.latest().unwrap().frame_id, 1);
    }

    #[test]
    fn audio_output_sink_preserves_latest_frame_summary() {
        let mut playback = AudioOutputPlayback::sink();

        playback
            .play(DecodedAudioFrame {
                frame_id: 5,
                sample_rate_hz: 48_000,
                channel_count: 1,
                sample_count: 3,
                capture_time_micros: 10,
                pcm: vec![1, 2, 3],
            })
            .unwrap();

        assert_eq!(playback.played_frames(), 1);
        assert_eq!(playback.latest().unwrap().frame_id, 5);
        assert_eq!(playback.latest().unwrap().sample_count, 3);
    }
}
