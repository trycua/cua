// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Whole-display targets on Windows: the monitors of the interactive
//! session (`EnumDisplayMonitors`) and a GDI capture loop per monitor.
//!
//! GDI (`BitBlt` of the screen DC with `CAPTUREBLT`) needs no GPU, so it works
//! on the basic display adapter a VM guest has (QEMU std VGA, Hyper-V), where
//! the per-window Windows.Graphics.Capture path may have no D3D device. It
//! reads what is on screen, the same pixels cua-driver's `get_desktop_state`
//! returns, so desktop coordinates on a frame are the coordinates desktop-scope
//! input resolves against. Frames are packed BGRA with opaque alpha.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_spacesd_provider_api::{
    CaptureEvent, CaptureLease, CaptureSink, OwnedFrame, PixelFormat, ProviderDisplay,
    ProviderError, ProviderErrorCode,
};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    EnumDisplayMonitors, GetDC, GetDIBits, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW, RGBQUAD, SRCCOPY,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

/// `MONITORINFOF_PRIMARY`.
const PRIMARY: u32 = 1;

struct Monitor {
    rect: RECT,
    name: String,
    primary: bool,
    scale: f64,
}

unsafe extern "system" fn collect(
    monitor: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` is the `Vec<Monitor>` passed by `monitors`.
    let out = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: a MONITORINFOEXW with its size set.
    if unsafe {
        GetMonitorInfoW(
            monitor,
            &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
        )
    }
    .as_bool()
    {
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        // SAFETY: out-pointers to locals.
        let scale =
            if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
                .is_ok()
                && dpi_x > 0
            {
                f64::from(dpi_x) / 96.0
            } else {
                1.0
            };
        let len = info
            .szDevice
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(info.szDevice.len());
        out.push(Monitor {
            rect: info.monitorInfo.rcMonitor,
            name: String::from_utf16_lossy(&info.szDevice[..len]),
            primary: info.monitorInfo.dwFlags & PRIMARY != 0,
            scale,
        });
    }
    true.into()
}

fn monitors() -> Vec<Monitor> {
    let mut out: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only runs during this call and writes to `out`.
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(collect),
            LPARAM(&mut out as *mut Vec<Monitor> as isize),
        );
    }
    // Primary first, then left to right: index 0 is the primary display.
    out.sort_by_key(|m| (!m.primary, m.rect.left, m.rect.top));
    out
}

/// The session's monitors; ids are indexes, the primary is "0".
pub(crate) fn displays() -> Vec<ProviderDisplay> {
    monitors()
        .into_iter()
        .enumerate()
        .map(|(index, m)| {
            let width = (m.rect.right - m.rect.left).max(0) as u32;
            let height = (m.rect.bottom - m.rect.top).max(0) as u32;
            ProviderDisplay {
                id: index.to_string(),
                name: m.name,
                primary: m.primary,
                bounds: (
                    f64::from(m.rect.left) / m.scale,
                    f64::from(m.rect.top) / m.scale,
                    f64::from(width) / m.scale,
                    f64::from(height) / m.scale,
                ),
                native_width_px: width,
                native_height_px: height,
                scale_factor: m.scale,
                refresh_rate_hz: 0,
            }
        })
        .collect()
}

fn capture_error(message: impl Into<String>) -> ProviderError {
    ProviderError::new(ProviderErrorCode::CaptureFailed, message)
}

/// One BGRA frame of the screen rectangle (physical pixels).
fn grab(x: i32, y: i32, width: u32, height: u32) -> Result<Vec<u8>, ProviderError> {
    let (w, h) = (width as i32, height as i32);
    // SAFETY: plain GDI calls on handles created and released here (the same
    // sequence as cua-driver's screen-region capture).
    unsafe {
        let screen = GetDC(HWND(std::ptr::null_mut()));
        if screen.is_invalid() {
            return Err(capture_error("GetDC(NULL) failed: no interactive desktop"));
        }
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let previous = SelectObject(memory, bitmap);
        let copied = BitBlt(memory, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // Negative: top-down rows.
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: width * height * 4,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };
        let mut out = vec![0u8; width as usize * height as usize * 4];
        let rows = GetDIBits(
            memory,
            bitmap,
            0,
            height,
            Some(out.as_mut_ptr() as *mut _),
            &mut info,
            DIB_RGB_COLORS,
        );
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory);
        ReleaseDC(HWND(std::ptr::null_mut()), screen);
        copied.map_err(|error| capture_error(format!("BitBlt failed: {error}")))?;
        if rows == 0 {
            return Err(capture_error("GetDIBits copied no rows"));
        }
        for pixel in out.as_chunks_mut::<4>().0 {
            pixel[3] = 0xff;
        }
        Ok(out)
    }
}

struct DisplayCaptureLease {
    stop: Arc<AtomicBool>,
}

impl CaptureLease for DisplayCaptureLease {
    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Streams display `display_id` at up to `max_fps` into `sink`.
pub(crate) fn start(
    display_id: &str,
    max_fps: u16,
    sink: Arc<dyn CaptureSink>,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    let index: usize = display_id
        .parse()
        .map_err(|_| ProviderError::new(ProviderErrorCode::TargetUnavailable, "unknown display"))?;
    let monitor = monitors().into_iter().nth(index).ok_or_else(|| {
        ProviderError::new(ProviderErrorCode::TargetUnavailable, "display is gone")
    })?;
    let (x, y) = (monitor.rect.left, monitor.rect.top);
    let width = (monitor.rect.right - monitor.rect.left).max(0) as u32;
    let height = (monitor.rect.bottom - monitor.rect.top).max(0) as u32;
    if width == 0 || height == 0 {
        return Err(capture_error("the display has no area"));
    }
    // Fail at start, not in the loop, when the desktop cannot be read.
    let first = grab(x, y, width, height)?;
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let interval = Duration::from_millis(1000 / u64::from(max_fps.clamp(1, 60)));
    std::thread::Builder::new()
        .name("rcdp-gdi-display".into())
        .spawn(move || {
            let epoch = Instant::now();
            let mut pending = Some(first);
            while !thread_stop.load(Ordering::Acquire) {
                let started = Instant::now();
                let bytes = match pending.take() {
                    Some(bytes) => Ok(bytes),
                    None => grab(x, y, width, height),
                };
                match bytes {
                    Ok(bytes) => sink.on_event(CaptureEvent::Frame(OwnedFrame {
                        bytes: Arc::from(bytes),
                        format: PixelFormat::Bgra8,
                        width_px: width,
                        height_px: height,
                        bytes_per_row: Some(width * 4),
                        capture_timestamp_us: epoch.elapsed().as_micros() as u64,
                        encode_duration_us: None,
                        codec_epoch: 1,
                        keyframe: true,
                    })),
                    // A locked or switched desktop (UAC, logon screen) has no
                    // readable pixels; report it and keep trying.
                    Err(_) => sink.on_event(CaptureEvent::Suspended("desktop_unreadable".into())),
                }
                if let Some(rest) = interval.checked_sub(started.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
        })
        .map_err(|error| capture_error(format!("failed to spawn capture thread: {error}")))?;
    Ok(Arc::new(DisplayCaptureLease { stop }))
}
