// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Windows monitor for `get_user_input_events`.
//!
//! Low-level mouse and keyboard hooks run on a thread of their own, started
//! by the first read. Each hook decodes the input, attributes it to the
//! window it went to, records it in the core log (which never makes the
//! hook wait on a reader), and passes the input on unchanged.
//!
//! The source comes from the hook's injected flag: input without it came
//! from a device. The driver tags its `SendInput` mouse input with
//! [`DRIVER_INPUT_TAG`] in `dwExtraInfo`, so that input reports as `driver`
//! and other injected input as `injected`. Its keyboard input is not tagged:
//! with the tag, WPF targets stopped reading typed text in the Windows E2E
//! harness. So the driver's own keys, like its synthetic pen and touch input,
//! report as `injected`. `SetCursorPos` raises no low-level hook event, so
//! the driver's cursor placements are not reported at all.

use cua_driver_core::user_input::{InputKind, InputSource, MouseButton};

/// `dwExtraInfo` on the mouse input the driver sends with `SendInput`.
pub(crate) const DRIVER_INPUT_TAG: usize = 0x6375_6164;

const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_XBUTTONDOWN: u32 = 0x020B;
const WM_XBUTTONUP: u32 = 0x020C;
const WM_MOUSEHWHEEL: u32 = 0x020E;

/// The kind and button of a low-level mouse hook message. `mouse_data` is
/// `MSLLHOOKSTRUCT::mouseData`, whose high word names the X button.
pub(crate) fn mouse_kind(
    message: u32,
    mouse_data: u32,
) -> Option<(InputKind, Option<MouseButton>)> {
    let x_button = || match mouse_data >> 16 {
        1 => Some(MouseButton::X1),
        2 => Some(MouseButton::X2),
        _ => None,
    };
    Some(match message {
        WM_MOUSEMOVE => (InputKind::MouseMove, None),
        WM_LBUTTONDOWN => (InputKind::MouseDown, Some(MouseButton::Left)),
        WM_LBUTTONUP => (InputKind::MouseUp, Some(MouseButton::Left)),
        WM_RBUTTONDOWN => (InputKind::MouseDown, Some(MouseButton::Right)),
        WM_RBUTTONUP => (InputKind::MouseUp, Some(MouseButton::Right)),
        WM_MBUTTONDOWN => (InputKind::MouseDown, Some(MouseButton::Middle)),
        WM_MBUTTONUP => (InputKind::MouseUp, Some(MouseButton::Middle)),
        WM_XBUTTONDOWN => (InputKind::MouseDown, x_button()),
        WM_XBUTTONUP => (InputKind::MouseUp, x_button()),
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => (InputKind::Wheel, None),
        _ => return None,
    })
}

pub(crate) fn key_kind(message: u32) -> Option<InputKind> {
    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(InputKind::KeyDown),
        WM_KEYUP | WM_SYSKEYUP => Some(InputKind::KeyUp),
        _ => None,
    }
}

pub(crate) fn source(injected: bool, extra_info: usize) -> InputSource {
    match (injected, extra_info) {
        (false, _) => InputSource::Hardware,
        (true, DRIVER_INPUT_TAG) => InputSource::Driver,
        (true, _) => InputSource::Injected,
    }
}

/// The `get_user_input_events` monitor on Windows.
pub struct WindowsUserInputMonitor;

impl cua_driver_core::user_input::UserInputMonitor for WindowsUserInputMonitor {
    fn ensure_started(
        &self,
        log: &std::sync::Arc<cua_driver_core::user_input::UserInputLog>,
    ) -> Result<(), String> {
        #[cfg(target_os = "windows")]
        {
            hook::ensure_started(log)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = log;
            Err("the Windows input hooks are only available when compiled for Windows".into())
        }
    }
}

#[cfg(target_os = "windows")]
mod hook {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex, OnceLock};

    use cua_driver_core::user_input::{now_ms, InputKind, ObservedInput, UserInputLog};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetAncestor, GetForegroundWindow, GetGUIThreadInfo,
        GetMessageW, GetWindowThreadProcessId, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, GUITHREADINFO, HC_ACTION, KBDLLHOOKSTRUCT,
        LLKHF_INJECTED, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, SystemParametersInfoW, MOUSEWHEEL_ROUTING_HYBRID,
        MOUSEWHEEL_ROUTING_MOUSE_POS, SPI_GETMOUSEWHEELROUTING,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };

    static LOG: OnceLock<Arc<UserInputLog>> = OnceLock::new();
    static RUNNING: AtomicBool = AtomicBool::new(false);
    static STARTING: Mutex<()> = Mutex::new(());

    pub(super) fn ensure_started(log: &Arc<UserInputLog>) -> Result<(), String> {
        let _starting = STARTING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if RUNNING.load(Ordering::Acquire) {
            return Ok(());
        }
        let _ = LOG.set(log.clone());
        let (ready, started) = mpsc::channel();
        std::thread::Builder::new()
            .name("cua-user-input-hooks".into())
            .spawn(move || run(ready))
            .map_err(|error| format!("could not start the hook thread: {error}"))?;
        // The thread reports once both hooks are installed or one failed; a
        // thread that ends without reporting drops `ready`. It sets RUNNING
        // itself, so its exit is never overwritten here.
        started
            .recv()
            .map_err(|_| "the hook thread ended before installing the hooks".to_owned())?
    }

    fn run(ready: mpsc::Sender<Result<(), String>>) {
        if let Err(error) = crate::dpi::use_per_monitor_v2_for_current_thread() {
            let _ = ready.send(Err(error));
            return;
        }
        let module = unsafe { GetModuleHandleW(None) }
            .ok()
            .map(|module| HINSTANCE(module.0));
        let mouse = match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0) } {
            Ok(hook) => hook,
            Err(error) => {
                let _ = ready.send(Err(format!("WH_MOUSE_LL: {error}")));
                return;
            }
        };
        let keyboard =
            match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0) } {
                Ok(hook) => hook,
                Err(error) => {
                    let _ = unsafe { UnhookWindowsHookEx(mouse) };
                    let _ = ready.send(Err(format!("WH_KEYBOARD_LL: {error}")));
                    return;
                }
            };
        RUNNING.store(true, Ordering::Release);
        let _ = ready.send(Ok(()));
        // Low-level hooks are called through this thread's message loop.
        // GetMessageW returns 0 on WM_QUIT and -1 on failure.
        let mut msg = MSG::default();
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = UnhookWindowsHookEx(mouse);
            let _ = UnhookWindowsHookEx(keyboard);
        }
        RUNNING.store(false, Ordering::Release);
    }

    fn owner(hwnd: HWND) -> (Option<u32>, Option<u64>) {
        if hwnd.0.is_null() {
            return (None, None);
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        ((pid != 0).then_some(pid), Some(hwnd.0 as u64))
    }

    fn wheel_routing() -> u32 {
        let mut routing = MOUSEWHEEL_ROUTING_MOUSE_POS;
        let _ = unsafe {
            SystemParametersInfoW(
                SPI_GETMOUSEWHEELROUTING,
                0,
                Some(&mut routing as *mut u32 as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        routing
    }

    /// A top-level window of a Windows Store app, which hybrid wheel routing
    /// sends the wheel to when it is under the pointer.
    fn is_store_app_window(hwnd: HWND) -> bool {
        let mut class = [0u16; 64];
        let length = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
        let class = String::from_utf16_lossy(&class[..length]);
        matches!(
            class.as_str(),
            "ApplicationFrameWindow" | "Windows.UI.Core.CoreWindow"
        )
    }

    /// The top-level window mouse input of `kind` at `point` goes to: the one
    /// holding mouse capture on the foreground thread, as during a drag; else
    /// the one under the pointer, except for a wheel that Windows routes to
    /// the focused window.
    fn mouse_target(kind: InputKind, point: POINT) -> HWND {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetGUIThreadInfo(0, &mut info) }.is_ok() && !info.hwndCapture.0.is_null() {
            return unsafe { GetAncestor(info.hwndCapture, GA_ROOT) };
        }
        let under = unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) };
        if kind != InputKind::Wheel {
            return under;
        }
        match wheel_routing() {
            MOUSEWHEEL_ROUTING_MOUSE_POS => under,
            MOUSEWHEEL_ROUTING_HYBRID if is_store_app_window(under) => under,
            _ => unsafe { GetForegroundWindow() },
        }
    }

    fn record(input: ObservedInput) {
        if let Some(log) = LOG.get() {
            log.record(input, now_ms());
        }
    }

    unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            if let Some((kind, button)) = super::mouse_kind(wparam.0 as u32, info.mouseData) {
                let source = super::source(info.flags & LLMHF_INJECTED != 0, info.dwExtraInfo);
                let (pid, window_id) = owner(mouse_target(kind, info.pt));
                record(ObservedInput {
                    kind,
                    source,
                    button,
                    x: Some(info.pt.x),
                    y: Some(info.pt.y),
                    pid,
                    window_id,
                });
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            if let Some(kind) = super::key_kind(wparam.0 as u32) {
                let (pid, window_id) = owner(unsafe { GetForegroundWindow() });
                record(ObservedInput {
                    kind,
                    source: super::source(info.flags.contains(LLKHF_INJECTED), info.dwExtraInfo),
                    button: None,
                    x: None,
                    y: None,
                    pid,
                    window_id,
                });
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_messages_decode_to_kind_and_button() {
        assert_eq!(
            mouse_kind(WM_MOUSEMOVE, 0),
            Some((InputKind::MouseMove, None))
        );
        assert_eq!(
            mouse_kind(WM_RBUTTONUP, 0),
            Some((InputKind::MouseUp, Some(MouseButton::Right)))
        );
        assert_eq!(
            mouse_kind(WM_XBUTTONDOWN, 2 << 16),
            Some((InputKind::MouseDown, Some(MouseButton::X2)))
        );
        assert_eq!(
            mouse_kind(WM_MOUSEHWHEEL, 120 << 16),
            Some((InputKind::Wheel, None))
        );
        assert_eq!(mouse_kind(WM_KEYDOWN, 0), None);
    }

    #[test]
    fn key_messages_decode_including_system_keys() {
        assert_eq!(key_kind(WM_SYSKEYDOWN), Some(InputKind::KeyDown));
        assert_eq!(key_kind(WM_KEYUP), Some(InputKind::KeyUp));
        assert_eq!(key_kind(WM_MOUSEMOVE), None);
    }

    #[test]
    fn only_tagged_injected_input_is_the_drivers() {
        assert_eq!(source(false, DRIVER_INPUT_TAG), InputSource::Hardware);
        assert_eq!(source(true, DRIVER_INPUT_TAG), InputSource::Driver);
        assert_eq!(source(true, 0), InputSource::Injected);
    }
}
