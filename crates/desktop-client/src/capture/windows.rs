use super::{CaptureConfig, CaptureFrame, CaptureSource, LatestFrameQueue, ScreenCapture};

#[cfg(target_os = "windows")]
use std::{ffi::c_void, mem, ptr};

#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{HANDLE, HWND, LPARAM, RECT},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetDeviceCaps,
        GetMonitorInfoW, GetWindowDC, HDC, HMONITOR, HORZRES, MONITORINFO, ReleaseDC, SRCCOPY,
        SelectObject, VERTRES,
    },
    UI::WindowsAndMessaging::{FindWindowW, GetWindowRect, IsWindowVisible, MONITORINFOF_PRIMARY},
};

#[cfg(target_os = "windows")]
const WINDOW_SOURCE_LABEL: &str = "window capture source";

#[cfg(target_os = "windows")]
const PRIMARY_MONITOR_SOURCE_LABEL: &str = "primary monitor";

#[cfg(target_os = "windows")]
const MONITOR_SOURCE_LABEL: &str = "monitor capture source";

#[cfg(not(target_os = "windows"))]
const PRIMARY_MONITOR_SOURCE_LABEL: &str = "primary monitor";

#[cfg(not(target_os = "windows"))]
const WINDOW_SOURCE_LABEL: &str = "window capture source";

#[cfg(not(target_os = "windows"))]
const MONITOR_SOURCE_LABEL: &str = "monitor capture source";

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct MonitorBounds {
    index: usize,
    rect: RECT,
    is_primary: bool,
}

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
        if let Some(frame) = self.queue.pop_latest() {
            return Ok(Some(frame));
        }
        Ok(Some(self.capture_now()?))
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

pub fn primary_monitor_size() -> anyhow::Result<(u32, u32)> {
    ensure_supported()?;
    primary_monitor_size_impl()
}

pub fn capture_source_size(source: &CaptureSource) -> anyhow::Result<(u32, u32)> {
    ensure_supported()?;
    capture_source_size_impl(source)
}

#[cfg(target_os = "windows")]
fn primary_monitor_size_impl() -> anyhow::Result<(u32, u32)> {
    with_screen_dc(|screen_dc| unsafe {
        let width = GetDeviceCaps(screen_dc, HORZRES as i32);
        let height = GetDeviceCaps(screen_dc, VERTRES as i32);
        validate_capture_dimensions(width, height, PRIMARY_MONITOR_SOURCE_LABEL)
    })
}

#[cfg(not(target_os = "windows"))]
fn primary_monitor_size_impl() -> anyhow::Result<(u32, u32)> {
    anyhow::bail!("Windows Graphics Capture is only available on Windows")
}

#[cfg(target_os = "windows")]
fn capture_source_size_impl(source: &CaptureSource) -> anyhow::Result<(u32, u32)> {
    match source {
        CaptureSource::PrimaryMonitor => primary_monitor_size_impl(),
        CaptureSource::Window { title, .. } => {
            let hwnd = find_window_by_title(title)?;
            window_capture_size(hwnd)
        }
        CaptureSource::Monitor { id } => monitor_bounds_by_id(id)?.size(),
    }
}

#[cfg(not(target_os = "windows"))]
fn capture_source_size_impl(_source: &CaptureSource) -> anyhow::Result<(u32, u32)> {
    anyhow::bail!("Windows Graphics Capture is only available on Windows")
}

impl WindowsGraphicsCapture {
    fn capture_now(&mut self) -> anyhow::Result<CaptureFrame> {
        match &self.source {
            CaptureSource::PrimaryMonitor => self.capture_primary_monitor(),
            CaptureSource::Window { title, .. } => self.capture_window(title.clone()),
            CaptureSource::Monitor { id } => self.capture_monitor(id.clone()),
        }
    }

    #[cfg(target_os = "windows")]
    fn capture_primary_monitor(&mut self) -> anyhow::Result<CaptureFrame> {
        with_screen_dc(|screen_dc| unsafe {
            let (width, height) = {
                let width = GetDeviceCaps(screen_dc, HORZRES as i32);
                let height = GetDeviceCaps(screen_dc, VERTRES as i32);
                validate_capture_dimensions(width, height, PRIMARY_MONITOR_SOURCE_LABEL)?
            };
            let bytes = capture_bgra_from_source_dc(
                screen_dc,
                width,
                height,
                PRIMARY_MONITOR_SOURCE_LABEL,
            )?;
            let frame = CaptureFrame::cpu_bgra(
                self.next_frame_id,
                width,
                height,
                unix_time_micros(),
                bytes,
            )?;
            self.next_frame_id = self.next_frame_id.saturating_add(1);
            Ok(frame)
        })
    }

    #[cfg(not(target_os = "windows"))]
    fn capture_primary_monitor(&mut self) -> anyhow::Result<CaptureFrame> {
        anyhow::bail!("Windows Graphics Capture is only available on Windows")
    }

    #[cfg(target_os = "windows")]
    fn capture_window(&mut self, title: String) -> anyhow::Result<CaptureFrame> {
        let hwnd = find_window_by_title(&title)?;
        let (width, height) = window_capture_size(hwnd)?;
        with_window_dc(hwnd, |window_dc| unsafe {
            let bytes = capture_bgra_from_source_dc(window_dc, width, height, WINDOW_SOURCE_LABEL)?;
            let frame = CaptureFrame::cpu_bgra(
                self.next_frame_id,
                width,
                height,
                unix_time_micros(),
                bytes,
            )?;
            self.next_frame_id = self.next_frame_id.saturating_add(1);
            Ok(frame)
        })
    }

    #[cfg(not(target_os = "windows"))]
    fn capture_window(&mut self, _title: String) -> anyhow::Result<CaptureFrame> {
        anyhow::bail!("Windows Graphics Capture is only available on Windows")
    }

    #[cfg(target_os = "windows")]
    fn capture_monitor(&mut self, id: String) -> anyhow::Result<CaptureFrame> {
        let monitor = monitor_bounds_by_id(&id)?;
        let (width, height) = monitor.size()?;
        with_screen_dc(|screen_dc| unsafe {
            let bytes = capture_bgra_from_source_region(
                screen_dc,
                monitor.rect.left,
                monitor.rect.top,
                width,
                height,
                MONITOR_SOURCE_LABEL,
            )?;
            let frame = CaptureFrame::cpu_bgra(
                self.next_frame_id,
                width,
                height,
                unix_time_micros(),
                bytes,
            )?;
            self.next_frame_id = self.next_frame_id.saturating_add(1);
            Ok(frame)
        })
    }

    #[cfg(not(target_os = "windows"))]
    fn capture_monitor(&mut self, _id: String) -> anyhow::Result<CaptureFrame> {
        anyhow::bail!("Windows Graphics Capture is only available on Windows")
    }
}

#[cfg(target_os = "windows")]
fn validate_capture_dimensions(
    width: i32,
    height: i32,
    source_label: &str,
) -> anyhow::Result<(u32, u32)> {
    if width <= 0 || height <= 0 {
        anyhow::bail!("{source_label} has invalid capture size {width}x{height}");
    }
    Ok((width as u32, height as u32))
}

#[cfg(target_os = "windows")]
impl MonitorBounds {
    fn size(&self) -> anyhow::Result<(u32, u32)> {
        validate_capture_dimensions(
            self.rect.right.saturating_sub(self.rect.left),
            self.rect.bottom.saturating_sub(self.rect.top),
            MONITOR_SOURCE_LABEL,
        )
    }
}

#[cfg(target_os = "windows")]
fn monitor_bounds_by_id(id: &str) -> anyhow::Result<MonitorBounds> {
    let monitors = enumerate_monitors()?;
    let id = id.trim();
    if id.eq_ignore_ascii_case("primary") {
        return monitors
            .into_iter()
            .find(|monitor| monitor.is_primary)
            .ok_or_else(|| anyhow::anyhow!("no primary monitor reported by Windows"));
    }

    let index = id
        .parse::<usize>()
        .map_err(|_| anyhow::anyhow!("monitor id must be a zero-based display index or primary"))?;
    monitors
        .into_iter()
        .find(|monitor| monitor.index == index)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "monitor index {} not found; available monitor indexes: {}",
                index,
                available_monitor_indexes()
            )
        })
}

#[cfg(target_os = "windows")]
fn available_monitor_indexes() -> String {
    match enumerate_monitors() {
        Ok(monitors) if !monitors.is_empty() => monitors
            .iter()
            .map(|monitor| {
                if monitor.is_primary {
                    format!("{} (primary)", monitor.index)
                } else {
                    monitor.index.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
        _ => "none".to_owned(),
    }
}

#[cfg(target_os = "windows")]
fn enumerate_monitors() -> anyhow::Result<Vec<MonitorBounds>> {
    unsafe extern "system" fn enum_monitor(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> i32 {
        let monitors = unsafe { &mut *(lparam as *mut Vec<MonitorBounds>) };
        let mut info = MONITORINFO {
            cbSize: mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: unsafe { mem::zeroed() },
            rcWork: unsafe { mem::zeroed() },
            dwFlags: 0,
        };
        if unsafe { GetMonitorInfoW(hmonitor, &mut info) } == 0 {
            return 1;
        }
        monitors.push(MonitorBounds {
            index: monitors.len(),
            rect: info.rcMonitor,
            is_primary: (info.dwFlags & MONITORINFOF_PRIMARY) != 0,
        });
        1
    }

    let mut monitors = Vec::new();
    let ok = unsafe {
        EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(enum_monitor),
            (&mut monitors as *mut Vec<MonitorBounds>) as LPARAM,
        )
    };
    if ok == 0 {
        anyhow::bail!("failed to enumerate display monitors");
    }
    if monitors.is_empty() {
        anyhow::bail!("Windows did not report any display monitors");
    }
    Ok(monitors)
}

#[cfg(target_os = "windows")]
fn find_window_by_title(title: &str) -> anyhow::Result<HWND> {
    let title = title.trim();
    if title.is_empty() {
        anyhow::bail!("window capture title cannot be empty");
    }
    let wide_title = wide_null(title);
    let hwnd = unsafe { FindWindowW(ptr::null(), wide_title.as_ptr()) };
    if hwnd.is_null() {
        anyhow::bail!("could not find a visible window with exact title {title:?}");
    }
    if unsafe { IsWindowVisible(hwnd) } == 0 {
        anyhow::bail!("window with title {title:?} is not visible");
    }
    Ok(hwnd)
}

#[cfg(target_os = "windows")]
fn window_capture_size(hwnd: HWND) -> anyhow::Result<(u32, u32)> {
    let mut rect = unsafe { mem::zeroed::<RECT>() };
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        anyhow::bail!("failed to query window capture bounds");
    }
    let width = rect.right.saturating_sub(rect.left);
    let height = rect.bottom.saturating_sub(rect.top);
    validate_capture_dimensions(width, height, WINDOW_SOURCE_LABEL)
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn with_screen_dc<T>(capture: impl FnOnce(HDC) -> anyhow::Result<T>) -> anyhow::Result<T> {
    unsafe {
        let hwnd = ptr::null_mut::<c_void>() as HWND;
        let screen_dc = GetDC(hwnd);
        if screen_dc.is_null() {
            anyhow::bail!("failed to acquire primary monitor device context");
        }
        let result = capture(screen_dc);
        ReleaseDC(hwnd, screen_dc);
        result
    }
}

#[cfg(target_os = "windows")]
fn with_window_dc<T>(
    hwnd: HWND,
    capture: impl FnOnce(HDC) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    unsafe {
        let window_dc = GetWindowDC(hwnd);
        if window_dc.is_null() {
            anyhow::bail!("failed to acquire window capture device context");
        }
        let result = capture(window_dc);
        ReleaseDC(hwnd, window_dc);
        result
    }
}

#[cfg(target_os = "windows")]
unsafe fn capture_bgra_from_source_dc(
    source_dc: HDC,
    width: u32,
    height: u32,
    source_label: &str,
) -> anyhow::Result<Vec<u8>> {
    unsafe { capture_bgra_from_source_region(source_dc, 0, 0, width, height, source_label) }
}

#[cfg(target_os = "windows")]
unsafe fn capture_bgra_from_source_region(
    source_dc: HDC,
    source_x: i32,
    source_y: i32,
    width: u32,
    height: u32,
    source_label: &str,
) -> anyhow::Result<Vec<u8>> {
    let memory_dc = unsafe { CreateCompatibleDC(source_dc) };
    if memory_dc.is_null() {
        anyhow::bail!("failed to create compatible capture device context");
    }

    let result = unsafe {
        capture_bgra_with_memory_dc(
            source_dc,
            memory_dc,
            source_x,
            source_y,
            width,
            height,
            source_label,
        )
    };
    unsafe {
        DeleteDC(memory_dc);
    }
    result
}

#[cfg(target_os = "windows")]
unsafe fn capture_bgra_with_memory_dc(
    source_dc: HDC,
    memory_dc: HDC,
    source_x: i32,
    source_y: i32,
    width: u32,
    height: u32,
    source_label: &str,
) -> anyhow::Result<Vec<u8>> {
    let buffer_len = CaptureFrame::bgra_byte_len(width, height)?;
    let mut bitmap_info = unsafe { mem::zeroed::<BITMAPINFO>() };
    bitmap_info.bmiHeader = BITMAPINFOHEADER {
        biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        biHeight: -(height as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: buffer_len.min(u32::MAX as usize) as u32,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };

    let mut bits = ptr::null_mut::<c_void>();
    let bitmap = unsafe {
        CreateDIBSection(
            memory_dc,
            &bitmap_info,
            DIB_RGB_COLORS,
            &mut bits,
            ptr::null_mut::<c_void>() as HANDLE,
            0,
        )
    };
    if bitmap.is_null() || bits.is_null() {
        anyhow::bail!("failed to allocate BGRA capture bitmap");
    }

    let old_object = unsafe { SelectObject(memory_dc, bitmap) };
    let result = if old_object.is_null() {
        Err(anyhow::anyhow!("failed to select capture bitmap"))
    } else if unsafe {
        BitBlt(
            memory_dc,
            0,
            0,
            width as i32,
            height as i32,
            source_dc,
            source_x,
            source_y,
            SRCCOPY,
        )
    } == 0
    {
        Err(anyhow::anyhow!("failed to copy {source_label} pixels"))
    } else {
        let mut bytes = vec![0; buffer_len];
        unsafe {
            ptr::copy_nonoverlapping(bits.cast::<u8>(), bytes.as_mut_ptr(), buffer_len);
        }
        Ok(bytes)
    };

    if !old_object.is_null() {
        unsafe {
            SelectObject(memory_dc, old_object);
        }
    }
    unsafe {
        DeleteObject(bitmap);
    }
    result
}

#[cfg(target_os = "windows")]
fn unix_time_micros() -> u64 {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros()
        .min(u64::MAX as u128) as u64
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

    #[test]
    fn primary_monitor_size_is_available_on_windows() {
        if is_supported() {
            let (width, height) = primary_monitor_size().unwrap();
            assert!(width > 0);
            assert!(height > 0);
        }
    }

    #[test]
    fn capture_source_size_uses_primary_monitor_path() {
        if is_supported() {
            assert_eq!(
                capture_source_size(&CaptureSource::PrimaryMonitor).unwrap(),
                primary_monitor_size().unwrap()
            );
        }
    }

    #[test]
    fn capture_source_size_accepts_monitor_index() {
        if is_supported() {
            let (width, height) =
                capture_source_size(&CaptureSource::Monitor { id: "0".to_owned() }).unwrap();
            assert!(width > 0);
            assert!(height > 0);
        }
    }
}
