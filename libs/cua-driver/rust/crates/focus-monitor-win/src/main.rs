/// focus-monitor-win — Windows equivalent of macOS FocusMonitorApp.
///
/// Creates a visible Win32 window and tracks three kinds of focus loss:
///
///   1. WM_ACTIVATE (wParam==WA_INACTIVE): the window loses activation.
///      Written to %TEMP%\focus_monitor_losses.txt
///
///   2. WM_KILLFOCUS: the window loses keyboard focus.
///      Written to %TEMP%\focus_monitor_key_losses.txt
///
/// Prints  FOCUS_PID=<pid>  on stdout at startup so the test harness can
/// discover the process, then prints  FOCUS_HWND=<hwnd_as_u64>  so tests
/// can target it with cua-driver tools.
///
/// Exits cleanly on WM_DESTROY.

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("focus-monitor-win is Windows-only.");
    std::process::exit(1);
}

#[cfg(target_os = "windows")]
mod win {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::*;

    // ── global loss counters ─────────────────────────────────────────────────
    static ACTIVATE_LOSSES: AtomicU32 = AtomicU32::new(0);
    static ACTIVATE_GAINS: AtomicU32 = AtomicU32::new(0);
    static KEY_LOSSES: AtomicU32 = AtomicU32::new(0);
    static KEY_GAINS: AtomicU32 = AtomicU32::new(0);

    fn loss_file() -> std::path::PathBuf {
        loss_path("focus_monitor_losses.txt")
    }
    fn gain_file() -> std::path::PathBuf {
        loss_path("focus_monitor_gains.txt")
    }
    fn key_loss_file() -> std::path::PathBuf {
        loss_path("focus_monitor_key_losses.txt")
    }
    fn key_gain_file() -> std::path::PathBuf {
        loss_path("focus_monitor_key_gains.txt")
    }

    fn loss_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(name);
        p
    }

    fn write_count(path: &std::path::Path, n: u32) {
        let _ = std::fs::write(path, n.to_string());
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_ACTIVATE => {
                // WA_INACTIVE == 0 in the low word of wParam; WA_ACTIVE == 1, WA_CLICKACTIVE == 2
                if (wparam.0 & 0xFFFF) == 0 {
                    let n = ACTIVATE_LOSSES.fetch_add(1, Ordering::SeqCst) + 1;
                    write_count(&loss_file(), n);
                } else {
                    let n = ACTIVATE_GAINS.fetch_add(1, Ordering::SeqCst) + 1;
                    write_count(&gain_file(), n);
                }
                let _ = InvalidateRect(hwnd, None, true);
            }
            WM_KILLFOCUS => {
                let n = KEY_LOSSES.fetch_add(1, Ordering::SeqCst) + 1;
                write_count(&key_loss_file(), n);
                let _ = InvalidateRect(hwnd, None, true);
            }
            WM_SETFOCUS => {
                let n = KEY_GAINS.fetch_add(1, Ordering::SeqCst) + 1;
                write_count(&key_gain_file(), n);
                let _ = InvalidateRect(hwnd, None, true);
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let act_l = ACTIVATE_LOSSES.load(Ordering::SeqCst);
                let act_g = ACTIVATE_GAINS.load(Ordering::SeqCst);
                let key_l = KEY_LOSSES.load(Ordering::SeqCst);
                let key_g = KEY_GAINS.load(Ordering::SeqCst);
                let text = wide(&format!(
                    "act: {act_l}L / {act_g}G   key: {key_l}L / {key_g}G   (should stay net 0)"
                ));
                let _ = TextOutW(hdc, 10, 10, &text);
                let _ = EndPaint(hwnd, &ps);
            }
            WM_DESTROY => {
                PostQuitMessage(0);
            }
            _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
        }
        LRESULT(0)
    }

    pub fn run() {
        unsafe {
            let class_name = wide("FocusMonitorWin");

            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: HINSTANCE(std::ptr::null_mut()),
                lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
                hbrBackground: HBRUSH(COLOR_WINDOW.0 as *mut _),
                ..Default::default()
            };
            RegisterClassW(&wc);

            let title = wide("Focus Monitor (cua-driver UX guard)");
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::PCWSTR(class_name.as_ptr()),
                windows::core::PCWSTR(title.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                600,
                140,
                None,
                None,
                HINSTANCE(std::ptr::null_mut()),
                None,
            )
            .expect("CreateWindowExW failed");

            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = UpdateWindow(hwnd).ok();

            // Write initial zeros so tests can read even before any event.
            write_count(&loss_file(), 0);
            write_count(&gain_file(), 0);
            write_count(&key_loss_file(), 0);
            write_count(&key_gain_file(), 0);

            // Signal the test harness via temp files (avoids pipe-blocking issues
            // when stdout is captured by the test runner in sandbox environments).
            let pid = GetCurrentProcessId();
            let hwnd_val = hwnd.0 as usize;
            let pid_file = std::env::temp_dir().join("focus_monitor_pid.txt");
            let hwnd_file = std::env::temp_dir().join("focus_monitor_hwnd.txt");
            let _ = std::fs::write(&pid_file, pid.to_string());
            let _ = std::fs::write(&hwnd_file, hwnd_val.to_string());
            // Also print to stdout as a secondary signal.
            println!("FOCUS_PID={pid}");
            println!("FOCUS_HWND={hwnd_val}");
            use std::io::Write;
            std::io::stdout().flush().ok();

            // Message loop.
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn main() {
    win::run();
}
