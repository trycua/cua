use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
};

pub fn native_windows() -> Vec<(u32, u64, String)> {
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let windows = &mut *(lparam.0 as *mut Vec<(u32, u64, String)>);
        let title_len = GetWindowTextLengthW(hwnd);
        if title_len > 0 {
            let mut title = vec![0u16; title_len as usize + 1];
            let copied = GetWindowTextW(hwnd, &mut title);
            if copied > 0 {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                windows.push((
                    pid,
                    hwnd.0 as u64,
                    String::from_utf16_lossy(&title[..copied as usize]),
                ));
            }
        }
        TRUE
    }

    let mut windows = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(callback),
            LPARAM(&mut windows as *mut Vec<(u32, u64, String)> as isize),
        );
    }
    windows
}
