// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Windows monitor for `get_user_input_events`.
//!
//! Low-level mouse and keyboard hooks run on a thread of their own, started
//! by the first read. Each hook decodes the input, attributes it to the
//! window it went to, and records it in the core log, then passes it on
//! unchanged.
//!
//! The source comes from the hook's injected flag: input without it came
//! from a device. The driver tags every `SendInput` event with
//! [`DRIVER_INPUT_TAG`] in `dwExtraInfo`. `SetCursorPos` cannot carry a tag,
//! so the driver notes each point it places the cursor at
//! ([`expect_cursor_placement`]), and an untagged injected move to that exact
//! point is counted as the driver's. Synthetic pen and touch injection carries
//! no tag either and reports as `injected`.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cua_driver_core::user_input::{InputKind, InputSource, MouseButton};

/// `dwExtraInfo` on every input the driver sends with `SendInput`.
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

/// How long a noted cursor placement waits for its hook event.
const PLACEMENT_TTL: Duration = Duration::from_secs(1);
const MAX_PLACEMENTS: usize = 32;

/// Points the driver is placing the cursor at with `SetCursorPos`.
#[derive(Default)]
pub(crate) struct Placements(VecDeque<(i32, i32, Instant)>);

impl Placements {
    pub(crate) fn expect(&mut self, x: i32, y: i32, now: Instant) {
        if self.0.len() == MAX_PLACEMENTS {
            self.0.pop_front();
        }
        self.0.push_back((x, y, now));
    }

    /// Whether a move to `(x, y)` is a placement the driver noted, which it
    /// then consumes.
    pub(crate) fn take(&mut self, x: i32, y: i32, now: Instant) -> bool {
        self.0
            .retain(|&(_, _, at)| now.saturating_duration_since(at) <= PLACEMENT_TTL);
        match self.0.iter().position(|&(px, py, _)| (px, py) == (x, y)) {
            Some(index) => {
                self.0.remove(index);
                true
            }
            None => false,
        }
    }
}

static PLACEMENTS: Mutex<Placements> = Mutex::new(Placements(VecDeque::new()));

fn placements() -> std::sync::MutexGuard<'static, Placements> {
    PLACEMENTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Note that the driver is about to place the cursor at `(x, y)` with
/// `SetCursorPos`, so the hook can count that move as the driver's.
pub(crate) fn expect_cursor_placement(x: i32, y: i32) {
    #[cfg(target_os = "windows")]
    if !hook::running() {
        return;
    }
    placements().expect(x, y, Instant::now());
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
    use std::time::{Duration, Instant};

    use cua_driver_core::user_input::{now_ms, InputKind, ObservedInput, UserInputLog};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetAncestor, GetForegroundWindow, GetMessageW,
        GetWindowThreadProcessId, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
        WindowFromPoint, GA_ROOT, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED, MSG,
        MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL,
    };

    static LOG: OnceLock<Arc<UserInputLog>> = OnceLock::new();
    static RUNNING: AtomicBool = AtomicBool::new(false);
    static STARTING: Mutex<()> = Mutex::new(());

    pub(super) fn running() -> bool {
        RUNNING.load(Ordering::Acquire)
    }

    pub(super) fn ensure_started(log: &Arc<UserInputLog>) -> Result<(), String> {
        let _starting = STARTING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if running() {
            return Ok(());
        }
        let _ = LOG.set(log.clone());
        let (ready, started) = mpsc::channel();
        std::thread::Builder::new()
            .name("cua-user-input-hooks".into())
            .spawn(move || run(ready))
            .map_err(|error| format!("could not start the hook thread: {error}"))?;
        started
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "the hook thread did not report back".to_owned())??;
        RUNNING.store(true, Ordering::Release);
        Ok(())
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
        let _ = ready.send(Ok(()));
        // Low-level hooks are called through this thread's message loop.
        let mut msg = MSG::default();
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
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

    fn record(input: ObservedInput) {
        if let Some(log) = LOG.get() {
            log.record(input, now_ms());
        }
    }

    unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            if let Some((kind, button)) = super::mouse_kind(wparam.0 as u32, info.mouseData) {
                let mut source = super::source(info.flags & LLMHF_INJECTED != 0, info.dwExtraInfo);
                if kind == InputKind::MouseMove
                    && source == super::InputSource::Injected
                    && info.dwExtraInfo == 0
                    && super::placements().take(info.pt.x, info.pt.y, Instant::now())
                {
                    source = super::InputSource::Driver;
                }
                let target = unsafe { GetAncestor(WindowFromPoint(info.pt), GA_ROOT) };
                let (pid, window_id) = owner(target);
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

    #[test]
    fn a_noted_placement_matches_one_move_to_its_point_within_its_lifetime() {
        let start = Instant::now();
        let mut placements = Placements::default();
        placements.expect(10, 20, start);
        assert!(!placements.take(10, 21, start));
        assert!(placements.take(10, 20, start));
        assert!(!placements.take(10, 20, start));

        placements.expect(10, 20, start);
        assert!(!placements.take(10, 20, start + PLACEMENT_TTL + Duration::from_millis(1)));
    }
}
