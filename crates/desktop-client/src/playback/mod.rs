use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::decode::{DecodedFrame, DecodedPixelFormat};

const MIN_WINDOW_RENDER_WIDTH: u32 = 640;

pub trait VideoPlayback {
    fn render(&mut self, frame: DecodedFrame) -> anyhow::Result<()>;
}

#[derive(Debug)]
pub enum FramePlayback {
    Latest(LatestFramePlayback),
    Window(WindowFramePlayback),
}

impl FramePlayback {
    pub fn latest() -> Self {
        Self::Latest(LatestFramePlayback::new())
    }

    pub fn window(title: &str) -> anyhow::Result<Self> {
        Ok(Self::Window(WindowFramePlayback::new(title)?))
    }

    pub fn rendered_frames(&self) -> u64 {
        match self {
            Self::Latest(playback) => playback.rendered_frames(),
            Self::Window(playback) => playback.rendered_frames(),
        }
    }

    pub fn latest_frame(&self) -> Option<&RenderedFrame> {
        match self {
            Self::Latest(playback) => playback.latest(),
            Self::Window(playback) => playback.latest(),
        }
    }
}

impl VideoPlayback for FramePlayback {
    fn render(&mut self, frame: DecodedFrame) -> anyhow::Result<()> {
        match self {
            Self::Latest(playback) => playback.render(frame),
            Self::Window(playback) => playback.render(frame),
        }
    }
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
        let pixel_bytes = validate_bgra_frame(&frame)?;
        self.record_rendered_frame(&frame, pixel_bytes);
        Ok(())
    }
}

impl LatestFramePlayback {
    fn record_rendered_frame(&mut self, frame: &DecodedFrame, pixel_bytes: usize) {
        self.rendered_frames = self.rendered_frames.saturating_add(1);
        self.latest = Some(RenderedFrame {
            frame_id: frame.frame_id,
            width: frame.width,
            height: frame.height,
            pixel_format: frame.pixel_format,
            pixel_bytes,
            render_time_micros: unix_time_micros(),
        });
    }
}

#[derive(Debug, Default)]
pub struct NullPlayback;

impl VideoPlayback for NullPlayback {
    fn render(&mut self, _frame: DecodedFrame) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct WindowFramePlayback {
    latest: LatestFramePlayback,
    window: NativeFrameWindow,
}

impl WindowFramePlayback {
    pub fn new(title: &str) -> anyhow::Result<Self> {
        Ok(Self {
            latest: LatestFramePlayback::new(),
            window: NativeFrameWindow::new(title)?,
        })
    }

    pub fn rendered_frames(&self) -> u64 {
        self.latest.rendered_frames()
    }

    pub fn latest(&self) -> Option<&RenderedFrame> {
        self.latest.latest()
    }
}

impl VideoPlayback for WindowFramePlayback {
    fn render(&mut self, frame: DecodedFrame) -> anyhow::Result<()> {
        let pixel_bytes = validate_bgra_frame(&frame)?;
        self.window.render(&frame)?;
        self.latest.record_rendered_frame(&frame, pixel_bytes);
        Ok(())
    }
}

fn validate_bgra_frame(frame: &DecodedFrame) -> anyhow::Result<usize> {
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
    Ok(expected_bytes)
}

fn display_size_for_frame(width: u32, height: u32) -> (i32, i32) {
    if width == 0 || height == 0 {
        return (
            MIN_WINDOW_RENDER_WIDTH as i32,
            MIN_WINDOW_RENDER_WIDTH as i32,
        );
    }
    if width >= MIN_WINDOW_RENDER_WIDTH {
        return (
            width.min(i32::MAX as u32) as i32,
            height.min(i32::MAX as u32) as i32,
        );
    }
    let scaled_height = (height as u64)
        .saturating_mul(MIN_WINDOW_RENDER_WIDTH as u64)
        .div_ceil(width as u64)
        .max(1)
        .min(i32::MAX as u64) as i32;
    (MIN_WINDOW_RENDER_WIDTH as i32, scaled_height)
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct NativeFrameWindow {
    hwnd: windows_sys::Win32::Foundation::HWND,
    class_name: Vec<u16>,
    width: i32,
    height: i32,
}

#[cfg(target_os = "windows")]
impl NativeFrameWindow {
    fn new(title: &str) -> anyhow::Result<Self> {
        use std::{ffi::c_void, ptr};

        use windows_sys::Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
                RegisterClassW, SW_SHOW, ShowWindow, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
            },
        };

        unsafe extern "system" fn window_proc(
            hwnd: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }

        let class_name = wide_null(&format!(
            "TeamViewFrameWindow-{}-{}",
            std::process::id(),
            unix_time_micros()
        ));
        let title = wide_null(title);
        unsafe {
            let hinstance = GetModuleHandleW(ptr::null());
            if hinstance.is_null() {
                anyhow::bail!("failed to get current module handle for render window");
            }
            let window_class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance as HINSTANCE,
                hIcon: ptr::null_mut(),
                hCursor: ptr::null_mut(),
                hbrBackground: ptr::null_mut(),
                lpszMenuName: ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };
            if RegisterClassW(&window_class) == 0 {
                anyhow::bail!("failed to register render window class");
            }
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                MIN_WINDOW_RENDER_WIDTH as i32,
                (MIN_WINDOW_RENDER_WIDTH * 9 / 16) as i32,
                ptr::null_mut(),
                ptr::null_mut(),
                hinstance,
                ptr::null::<c_void>(),
            );
            if hwnd.is_null() {
                anyhow::bail!("failed to create render window");
            }
            ShowWindow(hwnd, SW_SHOW);
            Ok(Self {
                hwnd,
                class_name,
                width: MIN_WINDOW_RENDER_WIDTH as i32,
                height: (MIN_WINDOW_RENDER_WIDTH * 9 / 16) as i32,
            })
        }
    }

    fn render(&mut self, frame: &DecodedFrame) -> anyhow::Result<()> {
        use std::{ffi::c_void, mem, ptr};

        use windows_sys::Win32::{
            Foundation::RECT,
            Graphics::Gdi::{
                COLORONCOLOR, DIB_RGB_COLORS, GetDC, ReleaseDC, SRCCOPY, SetStretchBltMode,
                StretchDIBits,
            },
            UI::WindowsAndMessaging::{GetClientRect, SWP_NOMOVE, SWP_NOZORDER, SetWindowPos},
        };

        self.pump_messages();
        let (target_width, target_height) = display_size_for_frame(frame.width, frame.height);
        if target_width != self.width || target_height != self.height {
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    ptr::null_mut(),
                    0,
                    0,
                    target_width,
                    target_height,
                    SWP_NOMOVE | SWP_NOZORDER,
                );
            }
            self.width = target_width;
            self.height = target_height;
        }

        let mut rect = unsafe { mem::zeroed::<RECT>() };
        let (dest_width, dest_height) = unsafe {
            if GetClientRect(self.hwnd, &mut rect) == 0 {
                (target_width, target_height)
            } else {
                (
                    (rect.right - rect.left).max(1),
                    (rect.bottom - rect.top).max(1),
                )
            }
        };
        let bitmap_info = bitmap_info_for_frame(frame);
        unsafe {
            let window_dc = GetDC(self.hwnd);
            if window_dc.is_null() {
                anyhow::bail!("failed to acquire render window device context");
            }
            SetStretchBltMode(window_dc, COLORONCOLOR);
            let copied_scan_lines = StretchDIBits(
                window_dc,
                0,
                0,
                dest_width,
                dest_height,
                0,
                0,
                frame.width as i32,
                frame.height as i32,
                frame.pixels.as_ptr().cast::<c_void>(),
                &bitmap_info,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
            ReleaseDC(self.hwnd, window_dc);
            if copied_scan_lines == 0 {
                anyhow::bail!("failed to draw decoded frame into render window");
            }
        }
        self.pump_messages();
        Ok(())
    }

    fn pump_messages(&self) {
        use std::mem;

        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
        };

        unsafe {
            let mut message = mem::zeroed::<MSG>();
            while PeekMessageW(&mut message, self.hwnd, 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for NativeFrameWindow {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(target_os = "windows")]
fn bitmap_info_for_frame(frame: &DecodedFrame) -> windows_sys::Win32::Graphics::Gdi::BITMAPINFO {
    use std::mem;

    use windows_sys::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFO, BITMAPINFOHEADER};

    let mut bitmap_info = unsafe { mem::zeroed::<BITMAPINFO>() };
    bitmap_info.bmiHeader = BITMAPINFOHEADER {
        biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: frame.width as i32,
        biHeight: -(frame.height as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: frame.pixels.len().min(u32::MAX as usize) as u32,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    bitmap_info
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(not(target_os = "windows"))]
#[derive(Debug)]
struct NativeFrameWindow;

#[cfg(not(target_os = "windows"))]
impl NativeFrameWindow {
    fn new(_title: &str) -> anyhow::Result<Self> {
        anyhow::bail!("native render window is only available on Windows")
    }

    fn render(&mut self, _frame: &DecodedFrame) -> anyhow::Result<()> {
        anyhow::bail!("native render window is only available on Windows")
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

    #[test]
    fn frame_playback_latest_preserves_summary_api() {
        let mut playback = FramePlayback::latest();

        playback
            .render(DecodedFrame {
                frame_id: 8,
                width: 1,
                height: 1,
                pixel_format: DecodedPixelFormat::Bgra8,
                pixels: Bytes::from_static(&[1, 2, 3, 255]),
            })
            .unwrap();

        assert_eq!(playback.rendered_frames(), 1);
        assert_eq!(playback.latest_frame().unwrap().frame_id, 8);
    }

    #[test]
    fn display_size_scales_tiny_frames_for_window_preview() {
        assert_eq!(display_size_for_frame(160, 90), (640, 360));
        assert_eq!(display_size_for_frame(1920, 1080), (1920, 1080));
        assert_eq!(display_size_for_frame(90, 160), (640, 1138));
    }
}
