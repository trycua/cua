//! Windows human-input capture backend.
//!
//! Installs low-level `WH_KEYBOARD_LL` / `WH_MOUSE_LL` hooks on a dedicated
//! thread that pumps messages (LL hooks are delivered on the installing
//! thread). Every callback enforces, in order:
//!   1. drop OS-injected events (our own actuation / replay) — `*_INJECTED`,
//!   2. drop events while the target window is not foreground (window-scoping),
//!   3. drop events the [`CaptureGate`] rejects (indicator not live/covering).
//! Only after all three may an event be buffered. The keyboard path feeds the
//! shared [`Coalescer`] so typing is redacted and coalesced.
//!
//! Single active capture at a time (the recorder is a daemon singleton), so
//! hook state lives in one process-global guarded by a mutex.

use std::sync::mpsc::Sender;
use std::sync::Mutex;

use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetDoubleClickTime, GetKeyboardState, ToUnicode, VIRTUAL_KEY, VK_CONTROL,
    VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW, GetSystemMetrics,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, HHOOK,
    KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, SM_CXDOUBLECLK, SM_CXDRAG,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN,
};

use crate::coalesce::{Coalescer, KeyInput};
use crate::event::{Button, HumanEvent, Mods};
use crate::gate::CaptureGate;
use crate::{CaptureConfig, CaptureError, InputCapture};

/// Process-global hook state. `None` when no capture is active.
static HOOK_STATE: Mutex<Option<HookState>> = Mutex::new(None);

struct HookState {
    target_hwnd: isize,
    gate: CaptureGate,
    sink: Sender<HumanEvent>,
    clock: Box<dyn Fn() -> u64 + Send>,
    coalescer: Coalescer,
    /// In-progress mouse press: (down point, button, t_ms).
    mouse_down: Option<(POINT, Button, u64)>,
    /// Last completed click for double-click detection: (point, t_ms).
    last_click: Option<(POINT, u64)>,
}

impl HookState {
    fn now(&self) -> u64 {
        (self.clock)()
    }
    fn emit(&self, ev: HumanEvent) {
        let _ = self.sink.send(ev);
    }
}

/// Is `hwnd` (or its root) the current foreground window? Window-scoping gate.
fn is_target_foreground(target: isize) -> bool {
    let fg = unsafe { GetForegroundWindow() };
    fg.0 as isize == target
}

fn current_mods() -> Mods {
    // High bit set => key down.
    let down = |vk: VIRTUAL_KEY| (unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000) != 0;
    Mods {
        ctrl: down(VK_CONTROL),
        alt: down(VK_MENU),
        shift: down(VK_SHIFT),
        meta: down(VK_LWIN) || down(VK_RWIN),
    }
}

// ── Mouse hook ───────────────────────────────────────────────────────────────

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        if let Some(info) = (lparam.0 as *const MSLLHOOKSTRUCT).as_ref() {
            // 1. drop injected (our own actuation / replay).
            let injected = info.flags & LLMHF_INJECTED != 0;
            if !injected {
                if let Ok(mut guard) = HOOK_STATE.try_lock() {
                    if let Some(st) = guard.as_mut() {
                        handle_mouse(st, wparam.0 as u32, info);
                    }
                }
            }
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

fn handle_mouse(st: &mut HookState, msg: u32, info: &MSLLHOOKSTRUCT) {
    // 2. window-scoping.
    if !is_target_foreground(st.target_hwnd) {
        return;
    }
    let pt = info.pt;
    let now = st.now();
    let (x, y) = (pt.x as f64, pt.y as f64);

    // 3. indicator gate (point-covered). Wheel + button events all positioned.
    if !st.gate.allow_at(now, x, y) {
        return;
    }

    match msg {
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            // High word of mouseData is a signed wheel delta (multiples of 120).
            let delta = ((info.mouseData >> 16) & 0xffff) as i16 as f64 / 120.0;
            let (dx, dy) = if msg == WM_MOUSEHWHEEL { (delta, 0.0) } else { (0.0, delta) };
            st.emit(HumanEvent::Scroll { x, y, dx, dy, t_ms: now });
        }
        WM_LBUTTONDOWN => st.mouse_down = Some((pt, Button::Left, now)),
        WM_RBUTTONDOWN => st.mouse_down = Some((pt, Button::Right, now)),
        WM_MBUTTONDOWN => st.mouse_down = Some((pt, Button::Middle, now)),
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => finish_button(st, pt, now),
        _ => {}
    }
}

fn finish_button(st: &mut HookState, up: POINT, now: u64) {
    let Some((down, button, _down_t)) = st.mouse_down.take() else {
        return;
    };
    let drag_px = unsafe { GetSystemMetrics(SM_CXDRAG) }.max(4);
    let moved = (up.x - down.x).abs() > drag_px || (up.y - down.y).abs() > drag_px;
    if moved {
        st.last_click = None;
        st.emit(HumanEvent::Drag {
            from: (down.x as f64, down.y as f64),
            to: (up.x as f64, up.y as f64),
            button,
            t_ms: now,
        });
        return;
    }

    // Double-click: same-ish point within the system double-click time/space.
    let dbl_time = unsafe { GetDoubleClickTime() } as u64;
    let dbl_px = unsafe { GetSystemMetrics(SM_CXDOUBLECLK) }.max(2);
    if button == Button::Left {
        if let Some((prev, prev_t)) = st.last_click.take() {
            if now.saturating_sub(prev_t) <= dbl_time
                && (up.x - prev.x).abs() <= dbl_px
                && (up.y - prev.y).abs() <= dbl_px
            {
                st.emit(HumanEvent::DoubleClick {
                    x: up.x as f64,
                    y: up.y as f64,
                    button,
                    t_ms: now,
                });
                return;
            }
        }
        st.last_click = Some((up, now));
    }
    st.emit(HumanEvent::Click { x: up.x as f64, y: up.y as f64, button, t_ms: now });
}

// ── Keyboard hook ────────────────────────────────────────────────────────────

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        if let Some(info) = (lparam.0 as *const KBDLLHOOKSTRUCT).as_ref() {
            let injected = info.flags.0 & LLKHF_INJECTED.0 != 0;
            let is_down = wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN;
            if !injected && is_down {
                if let Ok(mut guard) = HOOK_STATE.try_lock() {
                    if let Some(st) = guard.as_mut() {
                        handle_key(st, info.vkCode, info.scanCode);
                    }
                }
            }
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

fn handle_key(st: &mut HookState, vk: u32, scan: u32) {
    if !is_target_foreground(st.target_hwnd) {
        return;
    }
    let now = st.now();
    if !st.gate.allow(now) {
        return;
    }
    let mods = current_mods();
    let key = build_key_input(vk, scan, mods);
    for ev in st.coalescer.push(key, now) {
        st.emit(ev);
    }
}

/// Translate a VK code into a [`KeyInput`]: a text char for plain typing, or a
/// semantic name for command/navigation keys.
fn build_key_input(vk: u32, scan: u32, mods: Mods) -> KeyInput {
    let name = vk_name(vk);
    // A text char is produced only when no command modifier is held (Shift is
    // fine — it just changes case). This keeps Ctrl+C out of the text buffer.
    let text_char = if mods.is_command() { None } else { translate_char(vk, scan) };
    KeyInput {
        name: name.unwrap_or_else(|| {
            text_char.map(|c| c.to_string()).unwrap_or_else(|| format!("VK_{vk}"))
        }),
        text_char,
        mods,
    }
}

/// Semantic name for non-text keys; `None` for keys that should be treated as
/// text (letters/digits/punctuation resolved via [`translate_char`]).
fn vk_name(vk: u32) -> Option<String> {
    use windows::Win32::UI::Input::KeyboardAndMouse as k;
    let n = match VIRTUAL_KEY(vk as u16) {
        k::VK_RETURN => "Enter",
        k::VK_TAB => "Tab",
        k::VK_ESCAPE => "Escape",
        k::VK_BACK => "Backspace",
        k::VK_DELETE => "Delete",
        k::VK_LEFT => "ArrowLeft",
        k::VK_RIGHT => "ArrowRight",
        k::VK_UP => "ArrowUp",
        k::VK_DOWN => "ArrowDown",
        k::VK_HOME => "Home",
        k::VK_END => "End",
        k::VK_PRIOR => "PageUp",
        k::VK_NEXT => "PageDown",
        k::VK_INSERT => "Insert",
        _ => {
            // F1..F24
            if (k::VK_F1.0..=k::VK_F24.0).contains(&(vk as u16)) {
                return Some(format!("F{}", vk as u16 - k::VK_F1.0 + 1));
            }
            return None;
        }
    };
    Some(n.to_string())
}

/// Resolve the printable character a key produces under the current keyboard
/// layout, ignoring command modifiers. Returns `None` for non-printable keys.
fn translate_char(vk: u32, scan: u32) -> Option<char> {
    let mut kb = [0u8; 256];
    unsafe {
        let _ = GetKeyboardState(&mut kb);
    }
    // Keep Shift (for capitals/symbols) but strip Ctrl/Alt so ToUnicode yields
    // the base text char rather than a control code.
    kb[VK_CONTROL.0 as usize] = 0;
    kb[VK_MENU.0 as usize] = 0;
    let mut buf = [0u16; 8];
    let n = unsafe { ToUnicode(vk, scan, Some(&kb), &mut buf, 0) };
    if n == 1 {
        let c = char::from_u32(buf[0] as u32)?;
        if !c.is_control() {
            return Some(c);
        }
    }
    None
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

struct WindowsCapture {
    thread: Option<std::thread::JoinHandle<()>>,
    thread_id: u32,
}

impl InputCapture for WindowsCapture {
    fn stop(mut self: Box<Self>) {
        // Ask the message-pump thread to exit; it unhooks + flushes on the way
        // out (see the thread body). PostThreadMessage may race startup, so the
        // thread also tolerates being asked to quit before the pump begins.
        if self.thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for WindowsCapture {
    fn drop(&mut self) {
        // Defensive: if stop() was not called, still tear the hook down.
        if self.thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn start(
    cfg: CaptureConfig,
    gate: CaptureGate,
    sink: Sender<HumanEvent>,
    clock: impl Fn() -> u64 + Send + 'static,
) -> Result<Box<dyn InputCapture>, CaptureError> {
    // Refuse a second concurrent capture (single daemon recorder).
    {
        let mut guard = HOOK_STATE.lock().unwrap();
        if guard.is_some() {
            return Err(CaptureError::Hook("a capture is already active".into()));
        }
        *guard = Some(HookState {
            target_hwnd: cfg.window_id as isize,
            gate,
            sink,
            clock: Box::new(clock),
            coalescer: Coalescer::new(cfg.capture_raw_text),
            mouse_down: None,
            last_click: None,
        });
    }

    let (tx_id, rx_id) = std::sync::mpsc::channel::<Result<u32, String>>();
    let thread = std::thread::Builder::new()
        .name("input-capture-win".into())
        .spawn(move || pump(tx_id))
        .map_err(|e| CaptureError::Hook(e.to_string()))?;

    match rx_id.recv() {
        Ok(Ok(thread_id)) => Ok(Box::new(WindowsCapture { thread: Some(thread), thread_id })),
        Ok(Err(e)) => {
            *HOOK_STATE.lock().unwrap() = None;
            let _ = thread.join();
            Err(CaptureError::Hook(e))
        }
        Err(_) => {
            *HOOK_STATE.lock().unwrap() = None;
            let _ = thread.join();
            Err(CaptureError::Hook("capture thread exited during startup".into()))
        }
    }
}

/// Message-pump thread: install both hooks, report our thread id, pump until
/// WM_QUIT, then unhook and clear global state.
fn pump(report: Sender<Result<u32, String>>) {
    let kbd = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) };
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) };

    let (kbd, mouse) = match (kbd, mouse) {
        (Ok(k), Ok(m)) => (k, m),
        (k, m) => {
            unsafe {
                if let Ok(h) = k {
                    let _ = UnhookWindowsHookEx(h);
                }
                if let Ok(h) = m {
                    let _ = UnhookWindowsHookEx(h);
                }
            }
            let _ = report.send(Err("SetWindowsHookExW failed".into()));
            *HOOK_STATE.lock().unwrap() = None;
            return;
        }
    };

    let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    let _ = report.send(Ok(tid));

    // Standard message loop. LL hook callbacks fire on this thread between
    // GetMessage calls.
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break; // WM_QUIT (0) or error (-1)
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        let _ = UnhookWindowsHookEx(kbd);
        let _ = UnhookWindowsHookEx(mouse);
    }
    // Flush any pending text run, then clear global state.
    let mut guard = HOOK_STATE.lock().unwrap();
    if let Some(st) = guard.as_mut() {
        if let Some(ev) = st.coalescer.flush() {
            st.emit(ev);
        }
    }
    *guard = None;
}
