use bytes::Bytes;
use teamview_protocol::{codec::CodecId, frame::EncodedFrame};

use std::fmt;

use crate::capture::{CaptureFrame, CaptureFrameStorage, CapturePixelFormat};

use super::VideoEncoder;

const ANNEX_B_START_CODE: &[u8; 4] = &[0x00, 0x00, 0x00, 0x01];
const SYNTHETIC_SPS_MAGIC: &[u8; 4] = b"TVS1";
const SYNTHETIC_PPS_MAGIC: &[u8; 4] = b"TVP1";
const SYNTHETIC_FRAME_MAGIC: &[u8; 4] = b"TVF1";
const SYNTHETIC_PREVIEW_MAGIC: &[u8; 4] = b"TVB1";
const MAX_SYNTHETIC_PREVIEW_WIDTH: u32 = 160;
const MAX_SYNTHETIC_PREVIEW_HEIGHT: u32 = 90;
const NAL_SPS: u8 = 0x67;
const NAL_PPS: u8 = 0x68;
const NAL_IDR_SLICE: u8 = 0x65;
const NAL_NON_IDR_SLICE: u8 = 0x41;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H264EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub frames_per_second: u16,
    pub bitrate_bps: u32,
    pub synthetic_payload_bytes: usize,
}

impl Default for H264EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            frames_per_second: 30,
            bitrate_bps: 4_000_000,
            synthetic_payload_bytes: 512,
        }
    }
}

#[derive(Debug, Default)]
pub struct H264Encoder {
    pub config: H264EncoderConfig,
    pub keyframe_requested: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264VideoEncoderBackend {
    Synthetic,
    MediaFoundation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H264EncoderBackendStatus {
    pub backend: H264VideoEncoderBackend,
    pub available: bool,
    pub hardware: bool,
    pub detail: String,
}

#[derive(Debug)]
pub enum H264VideoEncoder {
    Synthetic(H264Encoder),
    MediaFoundation(MediaFoundationH264Encoder),
}

impl H264VideoEncoder {
    pub fn new(
        backend: H264VideoEncoderBackend,
        config: H264EncoderConfig,
    ) -> anyhow::Result<Self> {
        match backend {
            H264VideoEncoderBackend::Synthetic => Ok(Self::Synthetic(H264Encoder {
                config,
                keyframe_requested: false,
            })),
            H264VideoEncoderBackend::MediaFoundation => Ok(Self::MediaFoundation(
                MediaFoundationH264Encoder::new(config)?,
            )),
        }
    }
}

impl VideoEncoder for H264VideoEncoder {
    fn encode(
        &mut self,
        frame: CaptureFrame,
        stream_id: u32,
    ) -> anyhow::Result<Option<EncodedFrame>> {
        match self {
            Self::Synthetic(encoder) => encoder.encode(frame, stream_id),
            Self::MediaFoundation(encoder) => encoder.encode(frame, stream_id),
        }
    }

    fn request_keyframe(&mut self) {
        match self {
            Self::Synthetic(encoder) => encoder.request_keyframe(),
            Self::MediaFoundation(encoder) => encoder.request_keyframe(),
        }
    }

    fn update_bitrate(&mut self, bitrate_bps: u32) {
        match self {
            Self::Synthetic(encoder) => encoder.update_bitrate(bitrate_bps),
            Self::MediaFoundation(encoder) => encoder.update_bitrate(bitrate_bps),
        }
    }

    fn update_frame_rate(&mut self, frames_per_second: u16) {
        match self {
            Self::Synthetic(encoder) => encoder.update_frame_rate(frames_per_second),
            Self::MediaFoundation(encoder) => encoder.update_frame_rate(frames_per_second),
        }
    }

    fn update_resolution(&mut self, width: u32, height: u32) {
        match self {
            Self::Synthetic(encoder) => encoder.update_resolution(width, height),
            Self::MediaFoundation(encoder) => encoder.update_resolution(width, height),
        }
    }

    fn bitrate_bps(&self) -> u32 {
        match self {
            Self::Synthetic(encoder) => encoder.bitrate_bps(),
            Self::MediaFoundation(encoder) => encoder.bitrate_bps(),
        }
    }

    fn target_payload_bytes(&self) -> usize {
        match self {
            Self::Synthetic(encoder) => encoder.target_payload_bytes(),
            Self::MediaFoundation(encoder) => encoder.target_payload_bytes(),
        }
    }

    fn set_target_payload_bytes(&mut self, bytes: usize) {
        match self {
            Self::Synthetic(encoder) => encoder.set_target_payload_bytes(bytes),
            Self::MediaFoundation(encoder) => encoder.set_target_payload_bytes(bytes),
        }
    }
}

pub fn h264_encoder_backend_status(backend: H264VideoEncoderBackend) -> H264EncoderBackendStatus {
    match backend {
        H264VideoEncoderBackend::Synthetic => H264EncoderBackendStatus {
            backend,
            available: true,
            hardware: false,
            detail: "synthetic Annex B test encoder".to_owned(),
        },
        H264VideoEncoderBackend::MediaFoundation => media_foundation_h264_encoder_status(),
    }
}

#[derive(Debug, Clone)]
pub struct MediaFoundationH264Encoder {
    config: H264EncoderConfig,
    target_payload_bytes: usize,
    keyframe_requested: bool,
}

impl MediaFoundationH264Encoder {
    pub fn new(config: H264EncoderConfig) -> anyhow::Result<Self> {
        let status = media_foundation_h264_encoder_status();
        if !status.available {
            anyhow::bail!(
                "Media Foundation H.264 encoder is unavailable: {}",
                status.detail
            );
        }
        Ok(Self {
            target_payload_bytes: config.synthetic_payload_bytes,
            config,
            keyframe_requested: true,
        })
    }
}

impl VideoEncoder for H264Encoder {
    fn encode(
        &mut self,
        frame: CaptureFrame,
        stream_id: u32,
    ) -> anyhow::Result<Option<EncodedFrame>> {
        let is_keyframe = self.keyframe_requested || frame.frame_id == 1;
        self.keyframe_requested = false;

        let mut bytes = Vec::new();
        if is_keyframe {
            append_nal(
                &mut bytes,
                NAL_SPS,
                &synthetic_sps_payload(frame.width, frame.height, self.config.frames_per_second),
            );
            append_nal(&mut bytes, NAL_PPS, SYNTHETIC_PPS_MAGIC);
            append_nal(&mut bytes, NAL_IDR_SLICE, &synthetic_frame_payload(&frame));
        } else {
            append_nal(
                &mut bytes,
                NAL_NON_IDR_SLICE,
                &synthetic_frame_payload(&frame),
            );
        }
        while bytes.len() < self.config.synthetic_payload_bytes {
            bytes.push(
                frame
                    .frame_id
                    .wrapping_add(bytes.len() as u64)
                    .to_le_bytes()[0],
            );
        }

        Ok(Some(EncodedFrame {
            room_stream_id: stream_id,
            frame_id: frame.frame_id as u32,
            media_timestamp: frame.frame_id.saturating_mul(3_000),
            sender_capture_time_micros: frame.capture_time_micros,
            sender_clock_offset_micros: 0,
            sender_encode_done_time_micros: 0,
            sender_send_time_micros: 0,
            server_receive_time_micros: 0,
            server_send_time_micros: 0,
            codec: CodecId::H264,
            is_keyframe,
            bytes: Bytes::from(bytes),
        }))
    }

    fn request_keyframe(&mut self) {
        self.keyframe_requested = true;
    }

    fn update_bitrate(&mut self, bitrate_bps: u32) {
        self.config.bitrate_bps = bitrate_bps;
    }

    fn update_frame_rate(&mut self, frames_per_second: u16) {
        self.config.frames_per_second = frames_per_second.max(1);
    }

    fn update_resolution(&mut self, width: u32, height: u32) {
        self.config.width = width;
        self.config.height = height;
    }

    fn bitrate_bps(&self) -> u32 {
        self.config.bitrate_bps
    }

    fn target_payload_bytes(&self) -> usize {
        self.config.synthetic_payload_bytes
    }

    fn set_target_payload_bytes(&mut self, bytes: usize) {
        self.config.synthetic_payload_bytes = bytes;
    }
}

impl VideoEncoder for MediaFoundationH264Encoder {
    fn encode(
        &mut self,
        _frame: CaptureFrame,
        _stream_id: u32,
    ) -> anyhow::Result<Option<EncodedFrame>> {
        anyhow::bail!(
            "Media Foundation H.264 encoder was selected and detected, but frame submission is not wired yet"
        );
    }

    fn request_keyframe(&mut self) {
        self.keyframe_requested = true;
    }

    fn update_bitrate(&mut self, bitrate_bps: u32) {
        self.config.bitrate_bps = bitrate_bps;
    }

    fn update_frame_rate(&mut self, frames_per_second: u16) {
        self.config.frames_per_second = frames_per_second.max(1);
    }

    fn update_resolution(&mut self, width: u32, height: u32) {
        self.config.width = width;
        self.config.height = height;
        self.request_keyframe();
    }

    fn bitrate_bps(&self) -> u32 {
        self.config.bitrate_bps
    }

    fn target_payload_bytes(&self) -> usize {
        self.target_payload_bytes
    }

    fn set_target_payload_bytes(&mut self, bytes: usize) {
        self.target_payload_bytes = bytes;
    }
}

impl fmt::Display for H264VideoEncoderBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Synthetic => formatter.write_str("synthetic"),
            Self::MediaFoundation => formatter.write_str("media-foundation"),
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn media_foundation_h264_encoder_status() -> H264EncoderBackendStatus {
    H264EncoderBackendStatus {
        backend: H264VideoEncoderBackend::MediaFoundation,
        available: false,
        hardware: true,
        detail: "Media Foundation H.264 encoding is only available on Windows".to_owned(),
    }
}

#[cfg(target_os = "windows")]
fn media_foundation_h264_encoder_status() -> H264EncoderBackendStatus {
    match media_foundation::probe_hardware_h264_encoder_count() {
        Ok(count) if count > 0 => H264EncoderBackendStatus {
            backend: H264VideoEncoderBackend::MediaFoundation,
            available: true,
            hardware: true,
            detail: format!("{count} hardware H.264 encoder MFT(s) available"),
        },
        Ok(_) => H264EncoderBackendStatus {
            backend: H264VideoEncoderBackend::MediaFoundation,
            available: false,
            hardware: true,
            detail: "Media Foundation started, but no hardware H.264 encoder MFT was enumerated"
                .to_owned(),
        },
        Err(error) => H264EncoderBackendStatus {
            backend: H264VideoEncoderBackend::MediaFoundation,
            available: false,
            hardware: true,
            detail: error.to_string(),
        },
    }
}

#[cfg(target_os = "windows")]
mod media_foundation {
    use std::{ffi::c_void, mem, ptr};

    use anyhow::Context;
    use windows_sys::{
        Win32::{
            Foundation::HMODULE,
            System::LibraryLoader::{GetProcAddress, LoadLibraryA},
        },
        core::{GUID, HRESULT, IUnknown_Vtbl},
    };

    const MF_VERSION: u32 = 0x0002_0070;
    const MFSTARTUP_FULL: u32 = 0;
    const MFT_ENUM_FLAG_HARDWARE: u32 = 0x0000_0004;
    const MFT_ENUM_FLAG_SORTANDFILTER: u32 = 0x0000_0040;
    const MFT_CATEGORY_VIDEO_ENCODER: GUID =
        GUID::from_u128(0xf79eac7d_e545_4387_bdee_d647d7bde42a);
    const MF_MEDIA_TYPE_VIDEO: GUID = GUID::from_u128(0x73646976_0000_0010_8000_00aa00389b71);
    const MF_VIDEO_FORMAT_H264: GUID = GUID::from_u128(0x34363248_0000_0010_8000_00aa00389b71);

    type MFStartupFn = unsafe extern "system" fn(u32, u32) -> HRESULT;
    type MFShutdownFn = unsafe extern "system" fn() -> HRESULT;
    type MFTEnumExFn = unsafe extern "system" fn(
        GUID,
        u32,
        *const MftRegisterTypeInfo,
        *const MftRegisterTypeInfo,
        *mut *mut *mut c_void,
        *mut u32,
    ) -> HRESULT;
    type CoTaskMemFreeFn = unsafe extern "system" fn(*const c_void);

    #[repr(C)]
    struct MftRegisterTypeInfo {
        guid_major_type: GUID,
        guid_sub_type: GUID,
    }

    pub fn probe_hardware_h264_encoder_count() -> anyhow::Result<u32> {
        let mfplat = load_library(b"mfplat.dll\0").context("failed to load mfplat.dll")?;
        let startup: MFStartupFn = load_proc(mfplat, b"MFStartup\0")?;
        let shutdown: MFShutdownFn = load_proc(mfplat, b"MFShutdown\0")?;
        let enum_ex: MFTEnumExFn = load_proc(mfplat, b"MFTEnumEx\0")?;

        hr_result("MFStartup", unsafe { startup(MF_VERSION, MFSTARTUP_FULL) })?;
        let _guard = MediaFoundationShutdown { shutdown };

        let input_type = MftRegisterTypeInfo {
            guid_major_type: MF_MEDIA_TYPE_VIDEO,
            guid_sub_type: GUID::default(),
        };
        let output_type = MftRegisterTypeInfo {
            guid_major_type: MF_MEDIA_TYPE_VIDEO,
            guid_sub_type: MF_VIDEO_FORMAT_H264,
        };
        let mut activates: *mut *mut c_void = ptr::null_mut();
        let mut count = 0_u32;
        hr_result("MFTEnumEx", unsafe {
            enum_ex(
                MFT_CATEGORY_VIDEO_ENCODER,
                MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
                &input_type,
                &output_type,
                &mut activates,
                &mut count,
            )
        })?;

        unsafe {
            release_activates(activates, count);
            free_cotaskmem(activates.cast());
        }
        Ok(count)
    }

    struct MediaFoundationShutdown {
        shutdown: MFShutdownFn,
    }

    impl Drop for MediaFoundationShutdown {
        fn drop(&mut self) {
            unsafe {
                let _ = (self.shutdown)();
            }
        }
    }

    fn load_library(name: &'static [u8]) -> anyhow::Result<HMODULE> {
        let module = unsafe { LoadLibraryA(name.as_ptr()) };
        if module.is_null() {
            anyhow::bail!(
                "LoadLibraryA({}) failed",
                String::from_utf8_lossy(c_string_name(name))
            );
        }
        Ok(module)
    }

    fn load_proc<T: Copy>(module: HMODULE, name: &'static [u8]) -> anyhow::Result<T> {
        let proc = unsafe { GetProcAddress(module, name.as_ptr()) };
        let Some(proc) = proc else {
            anyhow::bail!(
                "GetProcAddress({}) failed",
                String::from_utf8_lossy(c_string_name(name))
            );
        };
        Ok(unsafe { mem::transmute_copy(&proc) })
    }

    fn c_string_name(bytes: &'static [u8]) -> &'static [u8] {
        bytes.strip_suffix(&[0]).unwrap_or(bytes)
    }

    fn hr_result(action: &str, hr: HRESULT) -> anyhow::Result<()> {
        if hr >= 0 {
            Ok(())
        } else {
            anyhow::bail!("{action} failed with HRESULT 0x{:08x}", hr as u32)
        }
    }

    unsafe fn release_activates(activates: *mut *mut c_void, count: u32) {
        if activates.is_null() {
            return;
        }
        for index in 0..count as usize {
            let activate = unsafe { *activates.add(index) };
            if activate.is_null() {
                continue;
            }
            let vtbl = unsafe { *(activate as *mut *mut IUnknown_Vtbl) };
            if !vtbl.is_null() {
                unsafe {
                    ((*vtbl).Release)(activate);
                }
            }
        }
    }

    unsafe fn free_cotaskmem(memory: *const c_void) {
        if memory.is_null() {
            return;
        }
        let Ok(ole32) = load_library(b"ole32.dll\0") else {
            return;
        };
        let Ok(free) = load_proc::<CoTaskMemFreeFn>(ole32, b"CoTaskMemFree\0") else {
            return;
        };
        unsafe {
            free(memory);
        }
    }
}

fn append_nal(bytes: &mut Vec<u8>, nal_header: u8, payload: &[u8]) {
    bytes.extend_from_slice(ANNEX_B_START_CODE);
    bytes.push(nal_header);
    bytes.extend_from_slice(&annex_b_escape_payload(payload));
}

fn annex_b_escape_payload(payload: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(payload.len());
    let mut consecutive_zeros = 0_u8;
    for &byte in payload {
        if consecutive_zeros >= 2 && byte <= 3 {
            escaped.push(0x03);
            consecutive_zeros = 0;
        }
        escaped.push(byte);
        if byte == 0 {
            consecutive_zeros = consecutive_zeros.saturating_add(1);
        } else {
            consecutive_zeros = 0;
        }
    }
    escaped
}

fn synthetic_sps_payload(width: u32, height: u32, frames_per_second: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(14);
    payload.extend_from_slice(SYNTHETIC_SPS_MAGIC);
    payload.extend_from_slice(&width.to_le_bytes());
    payload.extend_from_slice(&height.to_le_bytes());
    payload.extend_from_slice(&frames_per_second.to_le_bytes());
    payload
}

fn synthetic_frame_payload(frame: &CaptureFrame) -> Vec<u8> {
    let mut payload = Vec::with_capacity(16);
    payload.extend_from_slice(SYNTHETIC_FRAME_MAGIC);
    payload.extend_from_slice(&(frame.frame_id as u32).to_le_bytes());
    payload.extend_from_slice(&frame.width.to_le_bytes());
    payload.extend_from_slice(&frame.height.to_le_bytes());
    if let Some(preview) = synthetic_preview_payload(frame) {
        payload.extend_from_slice(&preview);
    }
    payload
}

fn synthetic_preview_payload(frame: &CaptureFrame) -> Option<Vec<u8>> {
    let CaptureFrameStorage::CpuBytes(source_pixels) = &frame.storage else {
        return None;
    };
    if frame.format != CapturePixelFormat::Bgra8 {
        return None;
    }
    let expected_len = CaptureFrame::bgra_byte_len(frame.width, frame.height).ok()?;
    if source_pixels.len() != expected_len {
        return None;
    }

    let (preview_width, preview_height) = preview_dimensions(frame.width, frame.height);
    let preview_pixels = downsample_bgra_nearest(
        source_pixels,
        frame.width,
        frame.height,
        preview_width,
        preview_height,
    )?;
    let mut payload = Vec::with_capacity(16 + preview_pixels.len());
    payload.extend_from_slice(SYNTHETIC_PREVIEW_MAGIC);
    payload.extend_from_slice(&preview_width.to_le_bytes());
    payload.extend_from_slice(&preview_height.to_le_bytes());
    payload.extend_from_slice(&(preview_pixels.len() as u32).to_le_bytes());
    payload.extend_from_slice(&preview_pixels);
    Some(payload)
}

fn preview_dimensions(width: u32, height: u32) -> (u32, u32) {
    if width <= MAX_SYNTHETIC_PREVIEW_WIDTH && height <= MAX_SYNTHETIC_PREVIEW_HEIGHT {
        return (width.max(1), height.max(1));
    }

    let height_by_width =
        (height as u64 * MAX_SYNTHETIC_PREVIEW_WIDTH as u64 / width.max(1) as u64) as u32;
    if height_by_width <= MAX_SYNTHETIC_PREVIEW_HEIGHT {
        return (MAX_SYNTHETIC_PREVIEW_WIDTH, height_by_width.max(1));
    }

    let width_by_height =
        (width as u64 * MAX_SYNTHETIC_PREVIEW_HEIGHT as u64 / height.max(1) as u64) as u32;
    (width_by_height.max(1), MAX_SYNTHETIC_PREVIEW_HEIGHT)
}

fn downsample_bgra_nearest(
    source_pixels: &[u8],
    source_width: u32,
    source_height: u32,
    preview_width: u32,
    preview_height: u32,
) -> Option<Vec<u8>> {
    let preview_len = CaptureFrame::bgra_byte_len(preview_width, preview_height).ok()?;
    let mut preview = Vec::with_capacity(preview_len);
    for y in 0..preview_height {
        let source_y = y as u64 * source_height as u64 / preview_height as u64;
        for x in 0..preview_width {
            let source_x = x as u64 * source_width as u64 / preview_width as u64;
            let offset = ((source_y * source_width as u64 + source_x) * 4) as usize;
            preview.extend_from_slice(source_pixels.get(offset..offset + 4)?);
        }
    }
    Some(preview)
}

#[cfg(test)]
mod tests {
    use crate::capture::CaptureFrame;

    use super::*;

    #[test]
    fn synthetic_encoder_outputs_protocol_frame() {
        let mut encoder = H264Encoder::default();
        let frame = CaptureFrame::metadata_only(7, 1280, 720, 123_456);

        let encoded = encoder.encode(frame, 9).unwrap().unwrap();

        assert_eq!(encoded.room_stream_id, 9);
        assert_eq!(encoded.frame_id, 7);
        assert_eq!(encoded.sender_capture_time_micros, 123_456);
        assert_eq!(encoded.codec, CodecId::H264);
        assert!(!encoded.bytes.is_empty());
    }

    #[test]
    fn synthetic_encoder_uses_configured_payload_size() {
        let mut encoder = H264Encoder::default();
        encoder.config.synthetic_payload_bytes = 2048;

        let encoded = encoder
            .encode(CaptureFrame::metadata_only(7, 1280, 720, 123_456), 9)
            .unwrap()
            .unwrap();

        assert_eq!(encoded.bytes.len(), 2048);
    }

    #[test]
    fn synthetic_encoder_keeps_required_metadata_when_payload_size_is_tiny() {
        let mut encoder = H264Encoder::default();
        encoder.config.synthetic_payload_bytes = 8;

        let encoded = encoder
            .encode(CaptureFrame::metadata_only(1, 1280, 720, 123_456), 9)
            .unwrap()
            .unwrap();

        assert!(encoded.bytes.len() > 8);
        assert!(
            encoded
                .bytes
                .windows(ANNEX_B_START_CODE.len())
                .any(|window| window == ANNEX_B_START_CODE)
        );
    }

    #[test]
    fn keyframe_request_affects_next_frame_only() {
        let mut encoder = H264Encoder::default();
        encoder.request_keyframe();

        let first = encoder
            .encode(CaptureFrame::metadata_only(2, 1280, 720, 1), 9)
            .unwrap()
            .unwrap();
        let second = encoder
            .encode(CaptureFrame::metadata_only(3, 1280, 720, 2), 9)
            .unwrap()
            .unwrap();

        assert!(first.is_keyframe);
        assert!(!second.is_keyframe);
    }

    #[test]
    fn synthetic_encoder_embeds_cpu_bgra_preview() {
        let mut encoder = H264Encoder::default();
        let pixels = (0_u8..32).collect::<Vec<_>>();
        let frame = CaptureFrame::cpu_bgra(1, 4, 2, 123_456, pixels.clone()).unwrap();

        let encoded = encoder.encode(frame, 9).unwrap().unwrap();

        assert!(
            encoded
                .bytes
                .windows(SYNTHETIC_PREVIEW_MAGIC.len())
                .any(|window| window == SYNTHETIC_PREVIEW_MAGIC)
        );
        assert!(
            encoded
                .bytes
                .windows(pixels.len())
                .any(|window| window == pixels.as_slice())
        );
    }

    #[test]
    fn preview_dimensions_preserve_aspect_without_upscaling() {
        assert_eq!(preview_dimensions(4, 2), (4, 2));
        assert_eq!(preview_dimensions(1920, 1080), (160, 90));
        assert_eq!(preview_dimensions(1080, 1920), (50, 90));
    }
}
