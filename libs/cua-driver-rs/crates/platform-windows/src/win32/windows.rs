//! Enumerate top-level windows on Windows.
//!
//! Uses EnumWindows + GetWindowTextW + GetWindowThreadProcessId.
//! Returns only visible, non-minimized windows with a non-empty title.

use std::sync::Mutex;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible,
};

#[derive(Debug, Clone)]
pub struct WindowInfo {
    /// HWND cast to u64 for serialization.
    pub hwnd: u64,
    /// pid owning the window.
    pub pid: u32,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

struct EnumState {
    filter_pid: Option<u32>,
    windows: Vec<WindowInfo>,
}

/// List top-level visible windows. If `filter_pid` is Some, only that process.
pub fn list_windows(filter_pid: Option<u32>) -> Vec<WindowInfo> {
    let state = Mutex::new(EnumState { filter_pid, windows: Vec::new() });
    let state_ptr = &state as *const Mutex<EnumState> as isize;
    unsafe {
        let _ = EnumWindows(Some(enum_windows_cb), LPARAM(state_ptr));
    }
    state.into_inner().unwrap().windows
}

unsafe extern "system" fn enum_windows_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let state = &*(lparam.0 as *const Mutex<EnumState>);

    // Skip invisible or minimized windows.
    if IsWindowVisible(hwnd).0 == 0 || IsIconic(hwnd).0 != 0 {
        return TRUE;
    }

    // Get pid.
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));

    // Filter by pid if requested.
    {
        let s = state.lock().unwrap();
        if let Some(fp) = s.filter_pid {
            if pid != fp { return TRUE; }
        }
    }

    // Get title (skip empty).
    let title_len = GetWindowTextLengthW(hwnd);
    if title_len == 0 { return TRUE; }
    let mut buf = vec![0u16; (title_len + 1) as usize];
    GetWindowTextW(hwnd, &mut buf);
    let title = {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    };
    if title.trim().is_empty() { return TRUE; }

    // Get bounds — prefer DWM extended frame bounds (includes shadow), fallback to GetWindowRect.
    let (x, y, w, h) = get_window_bounds(hwnd);

    state.lock().unwrap().windows.push(WindowInfo {
        hwnd: hwnd.0 as u64,
        pid,
        title,
        x,
        y,
        width: w,
        height: h,
    });

    TRUE
}

fn get_window_bounds(hwnd: HWND) -> (i32, i32, i32, i32) {
    unsafe {
        let mut rect = RECT::default();
        // Try DwmGetWindowAttribute for accurate bounds (excludes drop shadow on W11).
        let ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        if ok.is_err() {
            // Fallback to GetWindowRect.
            let _ = GetWindowRect(hwnd, &mut rect);
        }
        (rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top)
    }
}
