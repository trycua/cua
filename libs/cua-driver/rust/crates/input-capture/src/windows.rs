//! Windows human-input capture backend.
//!
//! Installs low-level `WH_KEYBOARD_LL` / `WH_MOUSE_LL` hooks on a dedicated
//! thread that pumps messages (LL hooks are delivered on the installing
//! thread). Every callback enforces, in order:
//!   1. drop OS-injected events (our own actuation / replay) — `*_INJECTED`,
//!   2. drop events while the target window is not foreground (window-scoping),
//!   3. drop events when indicator rendering is stale or the pointer is outside the target.
//! Only after all three may an event be buffered. The keyboard path feeds the
//! shared [`Coalescer`] so typing is redacted and coalesced.
//!
//! Single active capture at a time (the recorder is a daemon singleton), so
//! hook state lives in one process-global guarded by a mutex.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardState, ToUnicode, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU,
    VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetAncestor, GetForegroundWindow, GetMessageW,
    GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId, KillTimer, PostThreadMessageW,
    SetTimer, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, GA_ROOT, HHOOK,
    KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, SM_CXDRAG,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN, WM_TIMER,
};

use crate::coalesce::{Coalescer, KeyInput};
use crate::event::{Button, HumanEvent, Modifiers};
use crate::render_health::RenderHealth;
use crate::{CaptureConfig, CaptureError};

/// Process-global hook state. `None` when no capture is active.
static HOOK_STATE: Mutex<Option<HookState>> = Mutex::new(None);
const SCROLL_IDLE_MS: u64 = 150;

pub(crate) fn validate(config: &CaptureConfig) -> Result<(), CaptureError> {
    let target = HWND(config.window_id as *mut _);
    let mut actual_pid = 0;
    unsafe { GetWindowThreadProcessId(target, Some(&mut actual_pid)) };
    if config.pid <= 0 || actual_pid != config.pid as u32 {
        return Err(CaptureError::Hook(format!(
            "window {} does not belong to process {}",
            config.window_id, config.pid
        )));
    }
    if !is_target_foreground(config.window_id as isize) {
        return Err(CaptureError::Hook(format!(
            "window {} must be foreground before capture starts",
            config.window_id
        )));
    }
    Ok(())
}

struct HookState {
    target_hwnd: isize,
    health: RenderHealth,
    sink: SyncSender<HumanEvent>,
    started_at: Instant,
    coalescer: Coalescer,
    dropped_events: Arc<AtomicUsize>,
    pending_scroll: Option<PendingScroll>,
    /// In-progress mouse press: (down point, button, t_ms).
    mouse_down: Option<(POINT, Button, u64)>,
}

struct PendingScroll {
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    started_ms: u64,
    last_ms: u64,
}

impl HookState {
    fn now(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }
    fn emit(&self, event: HumanEvent) {
        if matches!(
            self.sink.try_send(event),
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_))
        ) {
            self.dropped_events.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Compare top-level roots so input sent to a child control remains in scope.
fn is_target_foreground(target: isize) -> bool {
    unsafe {
        let foreground = GetAncestor(GetForegroundWindow(), GA_ROOT);
        let target = GetAncestor(HWND(target as *mut _), GA_ROOT);
        !foreground.is_invalid() && foreground == target
    }
}

fn point_in_target(target: isize, point: POINT) -> bool {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(HWND(target as *mut _), &mut rect) }.is_ok()
        && point.x >= rect.left
        && point.x < rect.right
        && point.y >= rect.top
        && point.y < rect.bottom
}

fn current_modifiers() -> Modifiers {
    // High bit set => key down.
    let down = |vk: VIRTUAL_KEY| (unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000) != 0;
    Modifiers {
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
    let is_action = matches!(
        msg,
        WM_MOUSEWHEEL
            | WM_MOUSEHWHEEL
            | WM_LBUTTONDOWN
            | WM_RBUTTONDOWN
            | WM_MBUTTONDOWN
            | WM_LBUTTONUP
            | WM_RBUTTONUP
            | WM_MBUTTONUP
    );
    if !is_action {
        return;
    }

    let pt = info.pt;
    let now = st.now();
    if !is_target_foreground(st.target_hwnd)
        || !st.health.is_fresh(now)
        || !point_in_target(st.target_hwnd, pt)
    {
        flush_text(st);
        flush_scroll(st);
        st.mouse_down = None;
        return;
    }

    // Text occurred before this pointer action, even if its idle timer has not
    // fired yet. Emit it first so turn order remains causal.
    flush_text(st);
    if !matches!(msg, WM_MOUSEWHEEL | WM_MOUSEHWHEEL) {
        flush_scroll(st);
    }
    let (x, y) = (pt.x as f64, pt.y as f64);
    match msg {
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            // High word of mouseData is a signed wheel delta (multiples of 120).
            let delta = ((info.mouseData >> 16) & 0xffff) as i16 as f64 / 120.0;
            let (dx, dy) = if msg == WM_MOUSEHWHEEL {
                (delta, 0.0)
            } else {
                (0.0, delta)
            };
            queue_scroll(st, x, y, dx, dy, now);
        }
        WM_LBUTTONDOWN => st.mouse_down = Some((pt, Button::Left, now)),
        WM_RBUTTONDOWN => st.mouse_down = Some((pt, Button::Right, now)),
        WM_MBUTTONDOWN => st.mouse_down = Some((pt, Button::Middle, now)),
        WM_LBUTTONUP => finish_button(st, pt, Button::Left, now),
        WM_RBUTTONUP => finish_button(st, pt, Button::Right, now),
        WM_MBUTTONUP => finish_button(st, pt, Button::Middle, now),
        _ => {}
    }
}

fn queue_scroll(st: &mut HookState, x: f64, y: f64, dx: f64, dy: f64, now: u64) {
    if let Some(pending) = st.pending_scroll.as_mut() {
        if now.saturating_sub(pending.last_ms) < SCROLL_IDLE_MS {
            pending.x = x;
            pending.y = y;
            pending.dx += dx;
            pending.dy += dy;
            pending.last_ms = now;
            return;
        }
    }
    flush_scroll(st);
    st.pending_scroll = Some(PendingScroll {
        x,
        y,
        dx,
        dy,
        started_ms: now,
        last_ms: now,
    });
}

fn finish_button(st: &mut HookState, up: POINT, released: Button, now: u64) {
    let Some((down, button, _down_t)) = st.mouse_down.take() else {
        return;
    };
    if button != released {
        return;
    }
    let drag_px = unsafe { GetSystemMetrics(SM_CXDRAG) }.max(4);
    let moved = (up.x - down.x).abs() > drag_px || (up.y - down.y).abs() > drag_px;
    if moved {
        st.emit(HumanEvent::Drag {
            from: (down.x as f64, down.y as f64),
            to: (up.x as f64, up.y as f64),
            button,
            t_ms: now,
        });
        return;
    }

    // Preserve physical clicks. A later compiler can decide whether two clicks
    // form a higher-level double-click action.
    st.emit(HumanEvent::Click {
        x: up.x as f64,
        y: up.y as f64,
        button,
        t_ms: now,
    });
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
    let now = st.now();
    if !is_target_foreground(st.target_hwnd) || !st.health.is_fresh(now) {
        flush_text(st);
        flush_scroll(st);
        st.mouse_down = None;
        return;
    }
    flush_scroll(st);

    use windows::Win32::UI::Input::KeyboardAndMouse as key;
    if matches!(
        VIRTUAL_KEY(vk as u16),
        key::VK_CONTROL
            | key::VK_LCONTROL
            | key::VK_RCONTROL
            | key::VK_SHIFT
            | key::VK_LSHIFT
            | key::VK_RSHIFT
            | key::VK_MENU
            | key::VK_LMENU
            | key::VK_RMENU
            | key::VK_LWIN
            | key::VK_RWIN
    ) {
        return;
    }
    let modifiers = current_modifiers();
    let key = build_key_input(vk, scan, modifiers);
    for ev in st.coalescer.push(key, now) {
        st.emit(ev);
    }
}

fn flush_text(st: &mut HookState) {
    if let Some(event) = st.coalescer.flush() {
        st.emit(event);
    }
}

fn flush_scroll(st: &mut HookState) {
    if let Some(scroll) = st.pending_scroll.take() {
        st.emit(HumanEvent::Scroll {
            x: scroll.x,
            y: scroll.y,
            dx: scroll.dx,
            dy: scroll.dy,
            t_ms: scroll.started_ms,
        });
    }
}

/// Translate a VK code into a [`KeyInput`]: a text char for plain typing, or a
/// semantic name for command/navigation keys.
fn build_key_input(vk: u32, scan: u32, modifiers: Modifiers) -> KeyInput {
    let name = vk_name(vk);
    // A text char is produced only when no command modifier is held (Shift is
    // fine — it just changes case). This keeps Ctrl+C out of the text buffer.
    let text_char = if modifiers.is_command() {
        None
    } else {
        translate_char(vk, scan)
    };
    KeyInput {
        key: name.unwrap_or_else(|| {
            text_char
                .or_else(|| char::from_u32(vk).filter(char::is_ascii_alphanumeric))
                .map(|c| c.to_string())
                .unwrap_or_else(|| format!("VK_{vk}"))
        }),
        text_char,
        modifiers,
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
    // Bit 2 prevents ToUnicode from mutating the kernel dead-key state on
    // supported Windows versions.
    let n = unsafe { ToUnicode(vk, scan, Some(&kb), &mut buf, 1 << 2) };
    if n == 1 {
        let c = char::from_u32(buf[0] as u32)?;
        if !c.is_control() {
            return Some(c);
        }
    }
    None
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

pub(crate) struct Capture {
    thread: Option<std::thread::JoinHandle<()>>,
    thread_id: u32,
}

impl Capture {
    pub(crate) fn stop(mut self) {
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

impl Drop for Capture {
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

impl Capture {
    pub(crate) fn start(
        cfg: CaptureConfig,
        health: RenderHealth,
        sink: SyncSender<HumanEvent>,
        started_at: Instant,
        dropped_events: Arc<AtomicUsize>,
    ) -> Result<Self, CaptureError> {
        // Revalidate after indicator startup to close the target-replacement
        // race between preflight and hook installation.
        validate(&cfg)?;

        // Refuse a second concurrent capture (single daemon recorder).
        {
            let mut guard = HOOK_STATE.lock().unwrap();
            if guard.is_some() {
                return Err(CaptureError::Hook("a capture is already active".into()));
            }
            *guard = Some(HookState {
                target_hwnd: cfg.window_id as isize,
                health,
                sink,
                started_at,
                coalescer: Coalescer::new(),
                dropped_events,
                pending_scroll: None,
                mouse_down: None,
            });
        }

        let (tx_id, rx_id) = std::sync::mpsc::channel::<Result<u32, String>>();
        let thread = match std::thread::Builder::new()
            .name("input-capture-win".into())
            .spawn(move || pump(tx_id))
        {
            Ok(thread) => thread,
            Err(error) => {
                *HOOK_STATE.lock().unwrap() = None;
                return Err(CaptureError::Hook(error.to_string()));
            }
        };

        match rx_id.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread: Some(thread),
                thread_id,
            }),
            Ok(Err(e)) => {
                *HOOK_STATE.lock().unwrap() = None;
                let _ = thread.join();
                Err(CaptureError::Hook(e))
            }
            Err(_) => {
                *HOOK_STATE.lock().unwrap() = None;
                let _ = thread.join();
                Err(CaptureError::Hook(
                    "capture thread exited during startup".into(),
                ))
            }
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
    let timer_id = unsafe { SetTimer(HWND::default(), 0, 100, None) };
    let _ = report.send(Ok(tid));

    // Standard message loop. LL hook callbacks fire on this thread between
    // GetMessage calls.
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break; // WM_QUIT (0) or error (-1)
        }
        if msg.message == WM_TIMER {
            if let Ok(mut guard) = HOOK_STATE.try_lock() {
                if let Some(state) = guard.as_mut() {
                    let now = state.now();
                    if let Some(event) = state.coalescer.flush_if_idle(now) {
                        state.emit(event);
                    }
                    if state
                        .pending_scroll
                        .as_ref()
                        .is_some_and(|scroll| now.saturating_sub(scroll.last_ms) >= SCROLL_IDLE_MS)
                    {
                        flush_scroll(state);
                    }
                }
            }
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        if timer_id != 0 {
            let _ = KillTimer(HWND::default(), timer_id);
        }
        let _ = UnhookWindowsHookEx(kbd);
        let _ = UnhookWindowsHookEx(mouse);
    }
    // Flush any pending text run, then clear global state.
    let mut guard = HOOK_STATE.lock().unwrap();
    if let Some(st) = guard.as_mut() {
        flush_text(st);
        flush_scroll(st);
    }
    *guard = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_sink_increments_drop_count() {
        let (sink, _receiver) = std::sync::mpsc::sync_channel(0);
        let dropped_events = Arc::new(AtomicUsize::new(0));
        let state = HookState {
            target_hwnd: 0,
            health: RenderHealth::new(),
            sink,
            started_at: Instant::now(),
            coalescer: Coalescer::new(),
            dropped_events: dropped_events.clone(),
            pending_scroll: None,
            mouse_down: None,
        };
        state.emit(HumanEvent::Text { t_ms: 0 });
        assert_eq!(dropped_events.load(Ordering::Relaxed), 1);
    }
}
