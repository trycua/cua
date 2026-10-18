// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! RCDP input-echo fixture window.
//!
//! A bare Win32 window that end-to-end tests launch instead of a real
//! application, so background input never disturbs saved user state. It:
//!
//! - repaints an animated tick counter every 250 ms, so a window stream
//!   always has fresh frames to deliver;
//! - appends one JSON line per received input event (`click`, `char`,
//!   `key`, `wheel`, `activate`) to the log file given by `--log` or the
//!   `CUA_ENV_TEST_LOG` environment variable, including where the click landed
//!   in client, screen, and DWM-frame coordinates;
//! - logs a `geometry` line at startup with its window/client/frame bounds
//!   so a test can convert between coordinate spaces;
//! - shows the received-event count in its title.
//!
//! `--title <text>` names the window so tests can address one instance among
//! several.
//!
//! GUI subsystem: a console-subsystem binary would open a conhost window on
//! launch, and console windows activate themselves regardless of the
//! launcher's SW_SHOWNOACTIVATE.
#![windows_subsystem = "windows"]

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("cua-spacesd-test-pad only runs on Windows");
    std::process::exit(1);
}

#[cfg(target_os = "windows")]
fn main() {
    windows_main::run();
}

#[cfg(target_os = "windows")]
mod windows_main {
    use std::io::Write as _;
    use std::sync::Mutex;
    use std::time::Instant;

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, ClientToScreen, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
        InvalidateRect, SetBkMode, SetTextColor, DT_LEFT, DT_NOCLIP, PAINTSTRUCT, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW,
        GetWindowRect, PostQuitMessage, RegisterClassW, SetTimer, SetWindowTextW, ShowWindow,
        TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, MSG, SW_SHOWDEFAULT,
        WINDOW_EX_STYLE, WM_ACTIVATE, WM_CHAR, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN,
        WM_MBUTTONDOWN, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_TIMER, WNDCLASSW,
        WS_OVERLAPPEDWINDOW,
    };

    struct PadState {
        log: Option<std::fs::File>,
        started: Instant,
        events: u64,
        ticks: u64,
        clicks: u64,
        last_click: Option<(i32, i32)>,
        typed: String,
        title: String,
    }

    static STATE: Mutex<Option<PadState>> = Mutex::new(None);

    fn with_state<R>(f: impl FnOnce(&mut PadState) -> R) -> Option<R> {
        let mut guard = STATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.as_mut().map(f)
    }

    fn log_line(value: serde_json::Value) {
        with_state(|state| {
            let mut line = value;
            if let Some(object) = line.as_object_mut() {
                object.insert(
                    "t_ms".into(),
                    serde_json::json!(state.started.elapsed().as_millis() as u64),
                );
            }
            if let Some(file) = state.log.as_mut() {
                let _ = writeln!(file, "{line}");
                let _ = file.flush();
            }
        });
    }

    fn client_to_screen(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
        let mut point = POINT { x, y };
        unsafe {
            let _ = ClientToScreen(hwnd, &mut point);
        }
        (point.x, point.y)
    }

    fn frame_origin(hwnd: HWND) -> (i32, i32) {
        unsafe {
            let mut bounds = RECT::default();
            let dwm = DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut bounds as *mut _ as *mut _,
                std::mem::size_of::<RECT>() as u32,
            );
            if dwm.is_ok() {
                (bounds.left, bounds.top)
            } else {
                let mut rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rect);
                (rect.left, rect.top)
            }
        }
    }

    fn log_pointer_event(hwnd: HWND, kind: &str, button: &str, lparam: LPARAM) {
        let x = (lparam.0 & 0xffff) as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
        let (screen_x, screen_y) = client_to_screen(hwnd, x, y);
        let (frame_x, frame_y) = frame_origin(hwnd);
        log_line(serde_json::json!({
            "event": kind,
            "button": button,
            "client_x": x,
            "client_y": y,
            "screen_x": screen_x,
            "screen_y": screen_y,
            "frame_rel_x": screen_x - frame_x,
            "frame_rel_y": screen_y - frame_y,
        }));
        with_state(|state| {
            state.events += 1;
            if kind == "click" {
                state.clicks += 1;
                state.last_click = Some((x, y));
            }
        });
        refresh_title(hwnd);
        unsafe {
            let _ = InvalidateRect(hwnd, None, false);
        }
    }

    fn refresh_title(hwnd: HWND) {
        let title = with_state(|state| format!("{} [{} events]", state.title, state.events));
        if let Some(title) = title {
            let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                let _ = SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()));
            }
        }
    }

    fn log_geometry(hwnd: HWND) {
        unsafe {
            let mut window = RECT::default();
            let _ = GetWindowRect(hwnd, &mut window);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let (client_screen_x, client_screen_y) = client_to_screen(hwnd, 0, 0);
            let (frame_x, frame_y) = frame_origin(hwnd);
            log_line(serde_json::json!({
                "event": "geometry",
                "hwnd": hwnd.0 as u64,
                "window": [window.left, window.top, window.right, window.bottom],
                "client_origin": [client_screen_x, client_screen_y],
                "client_size": [client.right, client.bottom],
                "frame_origin": [frame_x, frame_y],
            }));
        }
    }

    extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            match message {
                WM_PAINT => {
                    let mut paint = PAINTSTRUCT::default();
                    let hdc = BeginPaint(hwnd, &mut paint);
                    let text = with_state(|state| {
                        let hue = (state.clicks % 6) as u32;
                        let color = [
                            0x00_202018u32,
                            0x00_182028,
                            0x00_282018,
                            0x00_182818,
                            0x00_281822,
                            0x00_222218,
                        ][hue as usize];
                        let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(color));
                        FillRect(hdc, &paint.rcPaint, brush);
                        let _ = DeleteObject(brush);
                        format!(
                            "RCDP test pad\ntick {}\nevents {}\nclicks {} last {:?}\ntyped: {}",
                            state.ticks, state.events, state.clicks, state.last_click, state.typed
                        )
                    })
                    .unwrap_or_default();
                    let mut wide: Vec<u16> = text.encode_utf16().collect();
                    let mut rect = RECT {
                        left: 12,
                        top: 12,
                        right: 640,
                        bottom: 480,
                    };
                    SetBkMode(hdc, TRANSPARENT);
                    SetTextColor(hdc, windows::Win32::Foundation::COLORREF(0x00_e8e8e8));
                    DrawTextW(hdc, &mut wide, &mut rect, DT_LEFT | DT_NOCLIP);
                    let _ = EndPaint(hwnd, &paint);
                    LRESULT(0)
                }
                WM_TIMER => {
                    with_state(|state| state.ticks += 1);
                    let _ = InvalidateRect(hwnd, None, false);
                    LRESULT(0)
                }
                WM_LBUTTONDOWN => {
                    log_pointer_event(hwnd, "click", "left", lparam);
                    LRESULT(0)
                }
                WM_RBUTTONDOWN => {
                    log_pointer_event(hwnd, "click", "right", lparam);
                    LRESULT(0)
                }
                WM_MBUTTONDOWN => {
                    log_pointer_event(hwnd, "click", "middle", lparam);
                    LRESULT(0)
                }
                WM_MOUSEWHEEL => {
                    let delta = ((wparam.0 >> 16) & 0xffff) as i16 as i32;
                    log_line(serde_json::json!({"event": "wheel", "delta": delta}));
                    with_state(|state| state.events += 1);
                    refresh_title(hwnd);
                    LRESULT(0)
                }
                WM_CHAR => {
                    let code = u32::try_from(wparam.0).unwrap_or(0);
                    let character = char::from_u32(code).unwrap_or('\u{fffd}');
                    log_line(serde_json::json!({"event": "char", "char": character.to_string()}));
                    with_state(|state| {
                        state.events += 1;
                        state.typed.push(character);
                        let excess = state.typed.len().saturating_sub(64);
                        if excess > 0 {
                            state.typed.drain(..excess);
                        }
                    });
                    refresh_title(hwnd);
                    let _ = InvalidateRect(hwnd, None, false);
                    LRESULT(0)
                }
                WM_KEYDOWN => {
                    log_line(serde_json::json!({"event": "key", "vk": wparam.0 as u32}));
                    LRESULT(0)
                }
                WM_ACTIVATE => {
                    log_line(serde_json::json!({
                        "event": "activate",
                        "state": (wparam.0 & 0xffff) as u32,
                    }));
                    LRESULT(0)
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
    }

    pub fn run() {
        let mut title = "RCDP Test Pad".to_owned();
        let mut log_path = std::env::var("CUA_ENV_TEST_LOG").ok();
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--title" => {
                    if let Some(value) = arguments.next() {
                        title = value;
                    }
                }
                "--log" => log_path = arguments.next(),
                _ => {}
            }
        }
        let log = log_path.as_deref().and_then(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        });
        *STATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PadState {
            log,
            started: Instant::now(),
            events: 0,
            ticks: 0,
            clicks: 0,
            last_click: None,
            typed: String::new(),
            title: title.clone(),
        });

        unsafe {
            let instance = GetModuleHandleW(None).expect("module handle");
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: w!("CuaEnvTestPad"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("CuaEnvTestPad"),
                PCWSTR(title_wide.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                520,
                400,
                None,
                None,
                instance,
                None,
            )
            .expect("window created");
            // SW_SHOWDEFAULT defers to STARTUPINFO's wShowWindow, so a
            // launcher using SW_SHOWNOACTIVATE keeps this window unfocused.
            let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);
            SetTimer(hwnd, 1, 250, None);
            log_geometry(hwnd);
            refresh_title(hwnd);

            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}
