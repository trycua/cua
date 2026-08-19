//! macOS human-input capture backend.
//!
//! A listen-only session event tap observes physical input. Every event is
//! gated on the exact target window remaining the frontmost normal window and
//! on a recently presented, visible AppKit recording border. Keyboard text is
//! reduced to a content-free marker before it reaches persistent state.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::coalesce::{Coalescer, KeyInput};
use crate::event::{Button, HumanEvent, Modifiers};
use crate::render_health::RenderHealth;
use crate::{CaptureConfig, CaptureError};

const SCROLL_IDLE_MS: u64 = 150;
const DRAG_THRESHOLD: f64 = 4.0;
const INDICATOR_FRAME_MS: u64 = 33;
const MAIN_QUEUE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy)]
struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Bounds {
    fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

#[derive(Debug, Clone, Copy)]
struct WindowSnapshot {
    window_id: u32,
    pid: i32,
    on_screen: bool,
    layer: i32,
    bounds: Bounds,
}

impl WindowSnapshot {
    #[cfg(test)]
    fn new(window_id: u32, pid: i32, on_screen: bool, layer: i32) -> Self {
        Self {
            window_id,
            pid,
            on_screen,
            layer,
            bounds: Bounds {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
        }
    }
}

fn validate_snapshot(
    pid: i32,
    window_id: u32,
    frontmost_pid: Option<i32>,
    windows: &[WindowSnapshot],
) -> Result<Bounds, String> {
    if pid <= 0 {
        return Err("target pid must be positive".into());
    }
    let target = windows
        .iter()
        .find(|window| window.window_id == window_id)
        .ok_or_else(|| format!("window {window_id} does not exist"))?;
    if target.pid != pid {
        return Err(format!(
            "window {window_id} does not belong to process {pid}"
        ));
    }
    if !target.on_screen
        || target.layer != 0
        || target.bounds.width <= 0.0
        || target.bounds.height <= 0.0
    {
        return Err(format!("window {window_id} is not a visible normal window"));
    }
    if frontmost_pid != Some(pid) {
        return Err(format!(
            "process {pid} must be foreground before capture starts"
        ));
    }
    let frontmost_window = windows
        .iter()
        .find(|window| window.pid == pid && window.layer == 0 && window.on_screen);
    if frontmost_window.map(|window| window.window_id) != Some(window_id) {
        return Err(format!(
            "window {window_id} must be the foreground window before capture starts"
        ));
    }
    Ok(target.bounds)
}

fn checked_identifiers(config: &CaptureConfig) -> Result<(i32, u32), CaptureError> {
    let pid = i32::try_from(config.pid)
        .map_err(|_| CaptureError::Hook("target pid is outside the macOS pid_t range".into()))?;
    if pid <= 0 {
        return Err(CaptureError::Hook("target pid must be positive".into()));
    }
    let window_id = u32::try_from(config.window_id).map_err(|_| {
        CaptureError::Hook("target window id is outside the CGWindowID range".into())
    })?;
    if window_id == 0 {
        return Err(CaptureError::Hook(
            "target window id must be positive".into(),
        ));
    }
    Ok((pid, window_id))
}

pub(crate) fn validate(config: &CaptureConfig) -> Result<(), CaptureError> {
    let (pid, window_id) = checked_identifiers(config)?;
    validate_target(pid, window_id)
        .map(|_| ())
        .map_err(CaptureError::Hook)
}

fn validate_target(pid: i32, window_id: u32) -> Result<Bounds, String> {
    validate_snapshot(pid, window_id, frontmost_pid(), &window_snapshots())
}

fn target_bounds_if_active(pid: i32, window_id: u32) -> Option<Bounds> {
    validate_target(pid, window_id).ok()
}

#[derive(Debug, Clone)]
enum Key {
    Printable,
    Named(&'static str),
}

#[derive(Debug, Clone)]
enum RawEvent {
    MouseDown { x: f64, y: f64, button: Button },
    MouseDragged { x: f64, y: f64, button: Button },
    MouseUp { x: f64, y: f64, button: Button },
    Scroll { x: f64, y: f64, dx: f64, dy: f64 },
    KeyDown { key: Key, modifiers: Modifiers },
}

#[derive(Debug, Clone, Copy)]
struct Gate {
    target_foreground: bool,
    indicator_fresh: bool,
    pointer_in_target: bool,
    secure_input: bool,
    injected: bool,
}

impl Gate {
    #[cfg(test)]
    fn active() -> Self {
        Self {
            target_foreground: true,
            indicator_fresh: true,
            pointer_in_target: true,
            secure_input: false,
            injected: false,
        }
    }
}

struct PendingScroll {
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    started_ms: u64,
    last_ms: u64,
}

struct EventProcessor {
    sink: SyncSender<HumanEvent>,
    dropped_events: Arc<AtomicUsize>,
    coalescer: Coalescer,
    mouse_down: Option<((f64, f64), Button)>,
    drag_to: Option<(f64, f64)>,
    pending_scroll: Option<PendingScroll>,
}

impl EventProcessor {
    fn new(sink: SyncSender<HumanEvent>, dropped_events: Arc<AtomicUsize>) -> Self {
        Self {
            sink,
            dropped_events,
            coalescer: Coalescer::new(),
            mouse_down: None,
            drag_to: None,
            pending_scroll: None,
        }
    }

    fn emit(&self, event: HumanEvent) {
        if matches!(
            self.sink.try_send(event),
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_))
        ) {
            self.dropped_events.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn process(&mut self, event: RawEvent, now: u64, gate: Gate) {
        if gate.injected || !gate.target_foreground || !gate.indicator_fresh {
            self.discard_pending();
            return;
        }
        if matches!(event, RawEvent::KeyDown { .. }) && gate.secure_input {
            self.discard_pending();
            return;
        }
        if !matches!(event, RawEvent::KeyDown { .. }) && !gate.pointer_in_target {
            self.discard_pending();
            return;
        }

        match event {
            RawEvent::KeyDown { key, modifiers } => {
                self.flush_scroll();
                let input = match key {
                    Key::Printable if !modifiers.is_command() => KeyInput {
                        text_char: Some('\0'),
                        key: "Printable".into(),
                        modifiers,
                    },
                    Key::Printable => KeyInput {
                        text_char: None,
                        key: "Printable".into(),
                        modifiers,
                    },
                    Key::Named(name) => KeyInput {
                        text_char: None,
                        key: name.into(),
                        modifiers,
                    },
                };
                for event in self.coalescer.push(input, now) {
                    self.emit(event);
                }
            }
            RawEvent::Scroll { x, y, dx, dy } => {
                self.flush_text();
                self.queue_scroll(x, y, dx, dy, now);
            }
            RawEvent::MouseDown { x, y, button } => {
                self.flush_text();
                self.flush_scroll();
                self.mouse_down = Some(((x, y), button));
                self.drag_to = None;
            }
            RawEvent::MouseDragged { x, y, button } => {
                if self
                    .mouse_down
                    .is_some_and(|(_, down_button)| down_button == button)
                {
                    self.drag_to = Some((x, y));
                }
            }
            RawEvent::MouseUp { x, y, button } => {
                self.flush_text();
                self.flush_scroll();
                let down = self.mouse_down.take();
                let drag_to = self.drag_to.take();
                match down {
                    Some((from, down_button)) if down_button == button => {
                        let to = drag_to.unwrap_or((x, y));
                        if (to.0 - from.0).abs() > DRAG_THRESHOLD
                            || (to.1 - from.1).abs() > DRAG_THRESHOLD
                        {
                            self.emit(HumanEvent::Drag {
                                from,
                                to: (x, y),
                                button,
                                t_ms: now,
                            });
                        } else {
                            self.emit(HumanEvent::Click {
                                x,
                                y,
                                button,
                                t_ms: now,
                            });
                        }
                    }
                    None => {}
                    _ => {}
                }
            }
        }
    }

    fn queue_scroll(&mut self, x: f64, y: f64, dx: f64, dy: f64, now: u64) {
        if let Some(pending) = self.pending_scroll.as_mut() {
            if now.saturating_sub(pending.last_ms) < SCROLL_IDLE_MS {
                pending.x = x;
                pending.y = y;
                pending.dx += dx;
                pending.dy += dy;
                pending.last_ms = now;
                return;
            }
        }
        self.flush_scroll();
        self.pending_scroll = Some(PendingScroll {
            x,
            y,
            dx,
            dy,
            started_ms: now,
            last_ms: now,
        });
    }

    fn flush(&mut self, now: u64) {
        if let Some(event) = self.coalescer.flush_if_idle(now) {
            self.emit(event);
        }
        if self
            .pending_scroll
            .as_ref()
            .is_some_and(|scroll| now.saturating_sub(scroll.last_ms) >= SCROLL_IDLE_MS)
        {
            self.flush_scroll();
        }
    }

    fn flush_text(&mut self) {
        if let Some(event) = self.coalescer.flush() {
            self.emit(event);
        }
    }

    fn flush_scroll(&mut self) {
        if let Some(scroll) = self.pending_scroll.take() {
            self.emit(HumanEvent::Scroll {
                x: scroll.x,
                y: scroll.y,
                dx: scroll.dx,
                dy: scroll.dy,
                t_ms: scroll.started_ms,
            });
        }
    }

    fn discard_pending(&mut self) {
        self.coalescer.discard();
        self.pending_scroll = None;
        self.mouse_down = None;
        self.drag_to = None;
    }
}

// CoreGraphics/CoreFoundation declarations are kept local so input-capture does
// not duplicate platform-macos's higher-level service architecture.
type CFTypeRef = *const c_void;
type CFRunLoopRef = *mut c_void;
type CFMachPortRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CGEventRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

type TapCallback = unsafe extern "C" fn(*mut c_void, u32, CGEventRef, *mut c_void) -> CGEventRef;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(
        option: u32,
        relative_to_window: u32,
    ) -> core_foundation::array::CFArrayRef;
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: TapCallback,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventGetIntegerValueField(event: CGEventRef, field: i32) -> i64;
    fn CGEventGetDoubleValueField(event: CGEventRef, field: i32) -> f64;
    fn CGEventKeyboardGetUnicodeString(
        event: CGEventRef,
        max: usize,
        actual: *mut usize,
        chars: *mut u16,
    );
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: CFTypeRef;
    fn CFMachPortCreateRunLoopSource(
        allocator: CFTypeRef,
        port: CFMachPortRef,
        order: isize,
    ) -> CFRunLoopSourceRef;
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(run_loop: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFTypeRef);
    fn CFRunLoopRunInMode(mode: CFTypeRef, seconds: f64, return_after_source_handled: bool) -> i32;
    fn CFRunLoopStop(run_loop: CFRunLoopRef);
    fn CFRetain(value: CFTypeRef) -> CFTypeRef;
    fn CFRelease(value: CFTypeRef);
}

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

const WINDOW_ON_SCREEN_EXCLUDE_DESKTOP: u32 = 1 | 16;
const SESSION_EVENT_TAP: u32 = 1;
const HEAD_INSERT: u32 = 0;
const LISTEN_ONLY: u32 = 1;
const EVENT_LEFT_DOWN: u32 = 1;
const EVENT_LEFT_UP: u32 = 2;
const EVENT_RIGHT_DOWN: u32 = 3;
const EVENT_RIGHT_UP: u32 = 4;
const EVENT_LEFT_DRAGGED: u32 = 6;
const EVENT_RIGHT_DRAGGED: u32 = 7;
const EVENT_KEY_DOWN: u32 = 10;
const EVENT_SCROLL: u32 = 22;
const EVENT_OTHER_DOWN: u32 = 25;
const EVENT_OTHER_UP: u32 = 26;
const EVENT_OTHER_DRAGGED: u32 = 27;
const EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xffff_fffe;
const EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xffff_ffff;
const FIELD_MOUSE_BUTTON: i32 = 3;
const FIELD_KEYCODE: i32 = 9;
const FIELD_SCROLL_AXIS_1: i32 = 11;
const FIELD_SCROLL_AXIS_2: i32 = 12;
const FIELD_SOURCE_PID: i32 = 41;

fn event_mask() -> u64 {
    [
        EVENT_LEFT_DOWN,
        EVENT_LEFT_UP,
        EVENT_RIGHT_DOWN,
        EVENT_RIGHT_UP,
        EVENT_LEFT_DRAGGED,
        EVENT_RIGHT_DRAGGED,
        EVENT_KEY_DOWN,
        EVENT_SCROLL,
        EVENT_OTHER_DOWN,
        EVENT_OTHER_UP,
        EVENT_OTHER_DRAGGED,
    ]
    .into_iter()
    .fold(0, |mask, event| mask | (1u64 << event))
}

fn window_snapshots() -> Vec<WindowSnapshot> {
    use core_foundation::{
        array::CFArray,
        base::{CFGetTypeID, TCFType},
        boolean::CFBoolean,
        dictionary::CFDictionary,
        number::CFNumber,
        string::CFString,
    };

    let raw = unsafe { CGWindowListCopyWindowInfo(WINDOW_ON_SCREEN_EXCLUDE_DESKTOP, 0) };
    if raw.is_null() {
        return Vec::new();
    }
    let array: CFArray<core_foundation::base::CFTypeRef> =
        unsafe { CFArray::wrap_under_create_rule(raw as _) };
    let dictionary_type = CFDictionary::<*const c_void, *const c_void>::type_id();
    array
        .iter()
        .filter_map(|item| {
            let item = *item;
            if unsafe { CFGetTypeID(item) } != dictionary_type {
                return None;
            }
            let dictionary: CFDictionary<*const c_void, *const c_void> =
                unsafe { CFDictionary::wrap_under_get_rule(item as _) };
            let number = |key: &str| -> i64 {
                let key = CFString::new(key);
                dictionary
                    .find(key.as_concrete_TypeRef() as *const c_void)
                    .and_then(|value| unsafe {
                        let value = *value;
                        (CFGetTypeID(value) == CFNumber::type_id())
                            .then(|| CFNumber::wrap_under_get_rule(value as _).to_i64())
                            .flatten()
                    })
                    .unwrap_or(0)
            };
            let boolean = |key: &str| -> bool {
                let key = CFString::new(key);
                dictionary
                    .find(key.as_concrete_TypeRef() as *const c_void)
                    .is_some_and(|value| unsafe {
                        let value = *value;
                        CFGetTypeID(value) == CFBoolean::type_id()
                            && bool::from(CFBoolean::wrap_under_get_rule(value as _))
                    })
            };
            let bounds_key = CFString::new("kCGWindowBounds");
            let bounds = dictionary
                .find(bounds_key.as_concrete_TypeRef() as *const c_void)
                .and_then(|value| unsafe {
                    let value = *value;
                    if CFGetTypeID(value) != dictionary_type {
                        return None;
                    }
                    let bounds: CFDictionary<*const c_void, *const c_void> =
                        CFDictionary::wrap_under_get_rule(value as _);
                    let component = |key: &str| {
                        let key = CFString::new(key);
                        bounds
                            .find(key.as_concrete_TypeRef() as *const c_void)
                            .and_then(|value| {
                                let value = *value;
                                (CFGetTypeID(value) == CFNumber::type_id())
                                    .then(|| CFNumber::wrap_under_get_rule(value as _).to_f64())
                                    .flatten()
                            })
                            .unwrap_or(0.0)
                    };
                    Some(Bounds {
                        x: component("X"),
                        y: component("Y"),
                        width: component("Width"),
                        height: component("Height"),
                    })
                })
                .unwrap_or(Bounds {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                });
            Some(WindowSnapshot {
                window_id: number("kCGWindowNumber") as u32,
                pid: number("kCGWindowOwnerPID") as i32,
                on_screen: boolean("kCGWindowIsOnscreen"),
                layer: number("kCGWindowLayer") as i32,
                bounds,
            })
        })
        .collect()
}

fn frontmost_pid() -> Option<i32> {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    unsafe {
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let app: *mut AnyObject = msg_send![workspace, frontmostApplication];
        (!app.is_null()).then(|| msg_send![app, processIdentifier])
    }
}

struct CallbackState {
    pid: i32,
    window_id: u32,
    health: RenderHealth,
    started_at: Instant,
    processor: EventProcessor,
    dropped_events: Arc<AtomicUsize>,
    continuity: TapContinuity,
    tap: usize,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct TapContinuity {
    lost: bool,
}

impl TapContinuity {
    fn latch_loss(&mut self, dropped_events: &AtomicUsize) {
        if !self.lost {
            self.lost = true;
            dropped_events.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn observe_event(&mut self, event_type: u32, dropped_events: &AtomicUsize) -> bool {
        let disabled = matches!(
            event_type,
            EVENT_TAP_DISABLED_BY_TIMEOUT | EVENT_TAP_DISABLED_BY_USER_INPUT
        );
        if disabled {
            self.latch_loss(dropped_events);
        }
        disabled
    }

    fn observe_enabled_state(&mut self, enabled: bool, dropped_events: &AtomicUsize) {
        if !enabled {
            self.latch_loss(dropped_events);
        }
    }

    #[cfg(test)]
    fn lost(&self) -> bool {
        self.lost
    }
}

fn recover_tap_with(
    stop: &AtomicBool,
    enable: impl FnOnce(),
    is_enabled: impl FnOnce() -> bool,
) -> bool {
    enable();
    let recovered = is_enabled();
    if !recovered {
        stop.store(true, Ordering::Release);
    }
    recovered
}

unsafe extern "C" fn event_callback(
    _proxy: *mut c_void,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    if user_info.is_null() {
        return event;
    }
    let state = &mut *(user_info as *mut CallbackState);
    if state
        .continuity
        .observe_event(event_type, &state.dropped_events)
    {
        state.processor.discard_pending();
        let tap = state.tap as CFMachPortRef;
        if tap.is_null() {
            state.stop.store(true, Ordering::Release);
        } else {
            recover_tap_with(
                &state.stop,
                || CGEventTapEnable(tap, true),
                || CGEventTapIsEnabled(tap),
            );
        }
        return event;
    }
    if event.is_null() {
        return event;
    }
    let now = state.started_at.elapsed().as_millis() as u64;
    let point = CGEventGetLocation(event);
    let bounds = target_bounds_if_active(state.pid, state.window_id);
    let source_pid = CGEventGetIntegerValueField(event, FIELD_SOURCE_PID);
    let gate = Gate {
        target_foreground: bounds.is_some(),
        indicator_fresh: state.health.is_fresh(now),
        pointer_in_target: bounds.is_some_and(|bounds| bounds.contains(point.x, point.y)),
        secure_input: IsSecureEventInputEnabled(),
        injected: source_pid > 0,
    };
    if let Some(raw) = raw_event(event_type, event, point) {
        state.processor.process(raw, now, gate);
    }
    event
}

unsafe fn raw_event(event_type: u32, event: CGEventRef, point: CGPoint) -> Option<RawEvent> {
    let button = || match CGEventGetIntegerValueField(event, FIELD_MOUSE_BUTTON) {
        0 => Button::Left,
        1 => Button::Right,
        _ => Button::Middle,
    };
    match event_type {
        EVENT_LEFT_DOWN | EVENT_RIGHT_DOWN | EVENT_OTHER_DOWN => Some(RawEvent::MouseDown {
            x: point.x,
            y: point.y,
            button: button(),
        }),
        EVENT_LEFT_UP | EVENT_RIGHT_UP | EVENT_OTHER_UP => Some(RawEvent::MouseUp {
            x: point.x,
            y: point.y,
            button: button(),
        }),
        EVENT_LEFT_DRAGGED | EVENT_RIGHT_DRAGGED | EVENT_OTHER_DRAGGED => {
            Some(RawEvent::MouseDragged {
                x: point.x,
                y: point.y,
                button: button(),
            })
        }
        EVENT_SCROLL => Some(RawEvent::Scroll {
            x: point.x,
            y: point.y,
            dx: CGEventGetDoubleValueField(event, FIELD_SCROLL_AXIS_2),
            dy: CGEventGetDoubleValueField(event, FIELD_SCROLL_AXIS_1),
        }),
        EVENT_KEY_DOWN => {
            let flags = CGEventGetFlags(event);
            let modifiers = Modifiers {
                shift: flags & 0x0002_0000 != 0,
                ctrl: flags & 0x0004_0000 != 0,
                alt: flags & 0x0008_0000 != 0,
                meta: flags & 0x0010_0000 != 0,
            };
            let keycode = CGEventGetIntegerValueField(event, FIELD_KEYCODE) as u16;
            let key = semantic_key(keycode, event, modifiers);
            Some(RawEvent::KeyDown { key, modifiers })
        }
        _ => None,
    }
}

unsafe fn semantic_key(keycode: u16, event: CGEventRef, modifiers: Modifiers) -> Key {
    if let Some(name) = key_name(keycode) {
        if modifiers.is_command() || is_navigation_name(name) {
            return Key::Named(name);
        }
    }
    let mut actual = 0usize;
    let mut unit = 0u16;
    CGEventKeyboardGetUnicodeString(event, 1, &mut actual, &mut unit);
    if actual > 0 && !char::from_u32(unit as u32).is_some_and(char::is_control) {
        if modifiers.is_command() {
            Key::Named(key_name(keycode).unwrap_or("Printable"))
        } else {
            Key::Printable
        }
    } else {
        Key::Named(key_name(keycode).unwrap_or("Unknown"))
    }
}

fn is_navigation_name(name: &str) -> bool {
    name.len() != 1 || !name.as_bytes()[0].is_ascii_alphanumeric()
}

fn key_name(keycode: u16) -> Option<&'static str> {
    Some(match keycode {
        0 => "a",
        1 => "s",
        2 => "d",
        3 => "f",
        4 => "h",
        5 => "g",
        6 => "z",
        7 => "x",
        8 => "c",
        9 => "v",
        11 => "b",
        12 => "q",
        13 => "w",
        14 => "e",
        15 => "r",
        16 => "y",
        17 => "t",
        18 => "1",
        19 => "2",
        20 => "3",
        21 => "4",
        22 => "6",
        23 => "5",
        25 => "9",
        26 => "7",
        28 => "8",
        29 => "0",
        31 => "o",
        32 => "u",
        34 => "i",
        35 => "p",
        37 => "l",
        38 => "j",
        40 => "k",
        45 => "n",
        46 => "m",
        36 => "Enter",
        48 => "Tab",
        49 => "Space",
        51 => "Backspace",
        53 => "Escape",
        55 => "Meta",
        56 => "Shift",
        57 => "CapsLock",
        58 => "Alt",
        59 => "Control",
        60 => "Shift",
        61 => "Alt",
        62 => "Control",
        63 => "Fn",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        99 => "F3",
        100 => "F8",
        101 => "F9",
        103 => "F11",
        105 => "F13",
        106 => "F16",
        107 => "F14",
        109 => "F10",
        111 => "F12",
        113 => "F15",
        114 => "Insert",
        115 => "Home",
        116 => "PageUp",
        117 => "Delete",
        118 => "F4",
        119 => "End",
        120 => "F2",
        121 => "PageDown",
        122 => "F1",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        _ => return None,
    })
}

trait RunLoopApi {
    unsafe fn stop(run_loop: usize);
    unsafe fn release(run_loop: usize);
}

struct CoreFoundationRunLoop;

impl RunLoopApi for CoreFoundationRunLoop {
    unsafe fn stop(run_loop: usize) {
        unsafe { CFRunLoopStop(run_loop as CFRunLoopRef) };
    }

    unsafe fn release(run_loop: usize) {
        unsafe { CFRelease((run_loop as CFRunLoopRef).cast()) };
    }
}

struct RetainedRunLoop<A: RunLoopApi = CoreFoundationRunLoop> {
    raw: usize,
    _api: std::marker::PhantomData<A>,
}

impl<A: RunLoopApi> RetainedRunLoop<A> {
    /// Takes ownership of one existing retain on `raw`.
    unsafe fn from_retained(raw: usize) -> Self {
        debug_assert_ne!(raw, 0);
        Self {
            raw,
            _api: std::marker::PhantomData,
        }
    }

    fn stop(&self) {
        unsafe { A::stop(self.raw) };
    }
}

impl<A: RunLoopApi> Drop for RetainedRunLoop<A> {
    fn drop(&mut self) {
        unsafe { A::release(self.raw) };
    }
}

struct CaptureLifecycle<A: RunLoopApi = CoreFoundationRunLoop> {
    stop: Arc<AtomicBool>,
    run_loop: RetainedRunLoop<A>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl<A: RunLoopApi> CaptureLifecycle<A> {
    fn shutdown(&mut self) {
        if self.thread.is_none() {
            return;
        }
        self.stop.store(true, Ordering::Release);
        self.run_loop.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl<A: RunLoopApi> Drop for CaptureLifecycle<A> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) struct Capture {
    lifecycle: CaptureLifecycle,
}

impl Capture {
    pub(crate) fn start(
        config: CaptureConfig,
        health: RenderHealth,
        sink: SyncSender<HumanEvent>,
        started_at: Instant,
        dropped_events: Arc<AtomicUsize>,
    ) -> Result<Self, CaptureError> {
        validate(&config)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let (report, ready) = std::sync::mpsc::channel::<Result<RetainedRunLoop, String>>();
        let (pid, window_id) = checked_identifiers(&config)?;
        let thread = std::thread::Builder::new()
            .name("input-capture-macos".into())
            .spawn(move || {
                run_event_tap(
                    pid,
                    window_id,
                    health,
                    sink,
                    started_at,
                    dropped_events,
                    thread_stop,
                    report,
                )
            })
            .map_err(|error| CaptureError::Hook(error.to_string()))?;
        match ready.recv() {
            Ok(Ok(run_loop)) => Ok(Self {
                lifecycle: CaptureLifecycle {
                    stop,
                    run_loop,
                    thread: Some(thread),
                },
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(CaptureError::Hook(error))
            }
            Err(_) => {
                let _ = thread.join();
                Err(CaptureError::Hook(
                    "event tap thread exited during startup".into(),
                ))
            }
        }
    }

    pub(crate) fn stop(mut self) {
        self.lifecycle.shutdown();
    }
}

#[allow(clippy::too_many_arguments)]
fn run_event_tap(
    pid: i32,
    window_id: u32,
    health: RenderHealth,
    sink: SyncSender<HumanEvent>,
    started_at: Instant,
    dropped_events: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    report: Sender<Result<RetainedRunLoop, String>>,
) {
    let processor_dropped_events = dropped_events.clone();
    let mut state = Box::new(CallbackState {
        pid,
        window_id,
        health,
        started_at,
        processor: EventProcessor::new(sink, processor_dropped_events),
        dropped_events,
        continuity: TapContinuity::default(),
        tap: 0,
        stop: stop.clone(),
    });
    unsafe {
        let tap = CGEventTapCreate(
            SESSION_EVENT_TAP,
            HEAD_INSERT,
            LISTEN_ONLY,
            event_mask(),
            event_callback,
            (&mut *state as *mut CallbackState).cast(),
        );
        if tap.is_null() {
            let _ = report.send(Err(
                "CGEventTapCreate failed (Accessibility permission may be unavailable)".into(),
            ));
            return;
        }
        state.tap = tap as usize;
        let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
        if source.is_null() {
            CFRelease(tap.cast());
            let _ = report.send(Err("failed to create event-tap run-loop source".into()));
            return;
        }
        let run_loop = CFRunLoopGetCurrent();
        CFRetain(run_loop.cast());
        CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
        CGEventTapEnable(tap, true);
        if !CGEventTapIsEnabled(tap) {
            CFRelease(source.cast());
            CFRelease(tap.cast());
            CFRelease(run_loop.cast());
            let _ = report.send(Err("CGEventTap could not be enabled".into()));
            return;
        }
        // The worker and owner need independent retains: failed tap recovery
        // can terminate this worker before Capture is stopped or dropped.
        CFRetain(run_loop.cast());
        let owner_run_loop = RetainedRunLoop::from_retained(run_loop as usize);
        if report.send(Ok(owner_run_loop)).is_err() {
            CFRelease(source.cast());
            CFRelease(tap.cast());
            CFRelease(run_loop.cast());
            return;
        }
        while !stop.load(Ordering::Acquire) {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, true);
            if !CGEventTapIsEnabled(tap) {
                state
                    .continuity
                    .observe_enabled_state(false, &state.dropped_events);
                state.processor.discard_pending();
                if !recover_tap_with(
                    &stop,
                    || CGEventTapEnable(tap, true),
                    || CGEventTapIsEnabled(tap),
                ) {
                    continue;
                }
            }
            let now = started_at.elapsed().as_millis() as u64;
            if target_bounds_if_active(pid, window_id).is_some()
                && state.health.is_fresh(now)
                && !IsSecureEventInputEnabled()
            {
                state.processor.flush(now);
            } else {
                state.processor.discard_pending();
            }
        }
        state.processor.flush_text();
        state.processor.flush_scroll();
        CFRelease(source.cast());
        CFRelease(tap.cast());
        CFRelease(run_loop.cast());
    }
}

pub(crate) fn spawn_indicator(
    window_id: u32,
    pid: i32,
    health: RenderHealth,
    started_at: Instant,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<std::thread::JoinHandle<()>> {
    // Reject before spawning an indicator worker if AppKit work cannot be
    // serviced. This keeps startup and its cleanup bounded when the daemon's
    // main thread is parked outside NSApplication.run().
    main_sync(|| ()).map_err(anyhow::Error::msg)?;
    std::thread::Builder::new()
        .name("recording-indicator-macos".into())
        .spawn(move || {
            let Ok(windows) = create_indicator_windows() else {
                health.clear();
                return;
            };
            if windows.len() != 4 {
                health.clear();
                let _ = close_indicator_windows(windows);
                return;
            }
            while !stop.load(Ordering::Acquire) {
                let visible = target_bounds_if_active(pid, window_id).is_some_and(|bounds| {
                    update_indicator_windows(&windows, bounds).unwrap_or(false)
                });
                if visible {
                    health.submitted(started_at.elapsed().as_millis() as u64);
                } else {
                    health.clear();
                    let _ = hide_indicator_windows(&windows);
                }
                std::thread::sleep(std::time::Duration::from_millis(INDICATOR_FRAME_MS));
            }
            health.clear();
            let _ = close_indicator_windows(windows);
        })
        .map_err(Into::into)
}

extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
}

extern "C" {
    fn pthread_main_np() -> i32;
}

type MainJob = Box<dyn FnOnce() + Send>;

unsafe extern "C" fn main_job_callback(context: *mut c_void) {
    let job = Box::from_raw(context as *mut MainJob);
    job();
}

fn dispatch_main(job: MainJob) {
    // Double-box the trait object so the C context is a thin pointer. The
    // callback owns it even if the caller's bounded wait has already expired.
    let context = Box::into_raw(Box::new(job)).cast();
    unsafe {
        dispatch_async_f(
            (&raw const _dispatch_main_q).cast(),
            context,
            main_job_callback,
        );
    }
}

fn main_sync_with<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    schedule: impl FnOnce(MainJob),
    timeout: Duration,
) -> Result<T, String> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let job_cancelled = cancelled.clone();
    schedule(Box::new(move || {
        if !job_cancelled.load(Ordering::Acquire) {
            let _ = sender.send(work());
        }
    }));
    match receiver.recv_timeout(timeout) {
        Ok(result) => Ok(result),
        Err(_) => {
            cancelled.store(true, Ordering::Release);
            Err("AppKit main queue is unavailable".to_owned())
        }
    }
}

fn main_sync<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    if unsafe { pthread_main_np() } != 0 {
        Ok(work())
    } else {
        main_sync_with(work, dispatch_main, MAIN_QUEUE_TIMEOUT)
    }
}

fn main_sync_cleanup_with<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    schedule: impl FnOnce(MainJob),
    timeout: Duration,
) -> Result<T, String> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    schedule(Box::new(move || {
        let _ = sender.send(work());
    }));
    receiver
        .recv_timeout(timeout)
        .map_err(|_| "AppKit main queue is unavailable during cleanup".to_owned())
}

fn main_sync_cleanup<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    if unsafe { pthread_main_np() } != 0 {
        Ok(work())
    } else {
        main_sync_cleanup_with(work, dispatch_main, MAIN_QUEUE_TIMEOUT)
    }
}

fn create_indicator_windows() -> Result<Vec<usize>, String> {
    main_sync(|| unsafe {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        let mut windows = Vec::with_capacity(4);
        for _ in 0..4 {
            let allocated: *mut AnyObject = msg_send![class!(NSWindow), alloc];
            let window: *mut AnyObject = msg_send![
                allocated,
                initWithContentRect: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0))
                styleMask: 0u64
                backing: 2u64
                defer: false
            ];
            if window.is_null() {
                break;
            }
            let red: *mut AnyObject = msg_send![
                class!(NSColor),
                colorWithCalibratedRed: 1.0f64 green: 0.08f64 blue: 0.04f64 alpha: 0.96f64
            ];
            let _: () = msg_send![window, setBackgroundColor: red];
            let _: () = msg_send![window, setOpaque: true];
            let _: () = msg_send![window, setHasShadow: false];
            let _: () = msg_send![window, setIgnoresMouseEvents: true];
            let _: () = msg_send![window, setLevel: 3i64];
            let _: () = msg_send![window, setCollectionBehavior: (1u64 | (1 << 4) | (1 << 8))];
            let _: () = msg_send![window, setReleasedWhenClosed: true];
            let _: () = msg_send![window, setHidesOnDeactivate: false];
            windows.push(window as usize);
        }
        windows
    })
}

fn appkit_frames(bounds: Bounds) -> [objc2_foundation::NSRect; 4] {
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    let main_height = unsafe { main_display_size().height };
    let x = bounds.x;
    let y = main_height - bounds.y - bounds.height;
    let thickness = 4.0;
    [
        NSRect::new(
            NSPoint::new(x - thickness, y + bounds.height),
            NSSize::new(bounds.width + 2.0 * thickness, thickness),
        ),
        NSRect::new(
            NSPoint::new(x - thickness, y - thickness),
            NSSize::new(bounds.width + 2.0 * thickness, thickness),
        ),
        NSRect::new(
            NSPoint::new(x - thickness, y),
            NSSize::new(thickness, bounds.height),
        ),
        NSRect::new(
            NSPoint::new(x + bounds.width, y),
            NSSize::new(thickness, bounds.height),
        ),
    ]
}

#[repr(C)]
struct CGSize {
    width: f64,
    height: f64,
}
#[repr(C)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> CGRect;
}
unsafe fn main_display_size() -> CGSize {
    CGDisplayBounds(CGMainDisplayID()).size
}

fn update_indicator_windows(windows: &[usize], bounds: Bounds) -> Result<bool, String> {
    let windows = windows.to_vec();
    main_sync(move || unsafe {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;
        let frames = appkit_frames(bounds);
        let mut all_visible = windows.len() == 4;
        for (window, frame) in windows.iter().zip(frames) {
            let window = *window as *mut AnyObject;
            let _: () = msg_send![window, setFrame: frame display: true];
            let _: () = msg_send![window, orderFrontRegardless];
            let _: () = msg_send![window, displayIfNeeded];
            let visible: bool = msg_send![window, isVisible];
            let occlusion: u64 = msg_send![window, occlusionState];
            all_visible &= visible && occlusion & 2 != 0;
        }
        all_visible
    })
}

fn hide_indicator_windows(windows: &[usize]) -> Result<(), String> {
    let windows = windows.to_vec();
    main_sync_cleanup(move || unsafe {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;
        for window in windows {
            let window = window as *mut AnyObject;
            let _: () = msg_send![window, orderOut: std::ptr::null_mut::<AnyObject>()];
        }
    })
}

fn close_indicator_windows(windows: Vec<usize>) -> Result<(), String> {
    main_sync_cleanup(move || unsafe {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;
        for window in windows {
            let window = window as *mut AnyObject;
            let _: () = msg_send![window, close];
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harness(
        capacity: usize,
    ) -> (
        EventProcessor,
        std::sync::mpsc::Receiver<HumanEvent>,
        Arc<AtomicUsize>,
    ) {
        let (sink, receiver) = std::sync::mpsc::sync_channel(capacity);
        let dropped = Arc::new(AtomicUsize::new(0));
        (
            EventProcessor::new(sink, dropped.clone()),
            receiver,
            dropped,
        )
    }

    #[test]
    fn validates_exact_visible_foreground_window_and_pid() {
        let windows = vec![
            WindowSnapshot::new(41, 800, true, 0),
            WindowSnapshot::new(42, 800, true, 0),
            WindowSnapshot::new(43, 900, true, 0),
        ];
        assert!(validate_snapshot(800, 41, Some(800), &windows).is_ok());
        assert!(validate_snapshot(800, 42, Some(800), &windows).is_err());
        assert!(validate_snapshot(900, 41, Some(800), &windows).is_err());
        assert!(validate_snapshot(800, 99, Some(800), &windows).is_err());
        let hidden = vec![WindowSnapshot::new(41, 800, false, 0)];
        assert!(validate_snapshot(800, 41, Some(800), &hidden).is_err());
    }

    #[test]
    fn stale_or_hidden_indicator_and_foreground_loss_pause_capture() {
        let (mut processor, receiver, _) = harness(8);
        processor.process(
            RawEvent::MouseDown {
                x: 10.0,
                y: 20.0,
                button: Button::Left,
            },
            0,
            Gate::active(),
        );
        let up = RawEvent::MouseUp {
            x: 10.0,
            y: 20.0,
            button: Button::Left,
        };
        processor.process(up.clone(), 1, Gate::active());
        processor.process(
            up.clone(),
            2,
            Gate {
                indicator_fresh: false,
                ..Gate::active()
            },
        );
        processor.process(
            up,
            3,
            Gate {
                target_foreground: false,
                ..Gate::active()
            },
        );
        assert_eq!(receiver.try_iter().count(), 1);
    }

    #[test]
    fn printable_text_is_redacted_and_commands_keep_key_and_modifiers() {
        let (mut processor, receiver, _) = harness(8);
        processor.process(
            RawEvent::KeyDown {
                key: Key::Printable,
                modifiers: Modifiers::default(),
            },
            1,
            Gate::active(),
        );
        processor.process(
            RawEvent::KeyDown {
                key: Key::Printable,
                modifiers: Modifiers::default(),
            },
            2,
            Gate::active(),
        );
        processor.process(
            RawEvent::KeyDown {
                key: Key::Named("c"),
                modifiers: Modifiers {
                    meta: true,
                    ..Default::default()
                },
            },
            3,
            Gate::active(),
        );
        let events: Vec<_> = receiver.try_iter().collect();
        assert_eq!(events[0], HumanEvent::Text { t_ms: 1 });
        assert!(
            matches!(&events[1], HumanEvent::Key { key, modifiers, .. } if key == "c" && modifiers.meta)
        );
        assert!(!format!("{events:?}").contains("Printable"));
    }

    #[test]
    fn click_drag_and_scroll_are_coalesced_semantically() {
        let (mut processor, receiver, _) = harness(8);
        processor.process(
            RawEvent::MouseDown {
                x: 1.0,
                y: 2.0,
                button: Button::Left,
            },
            1,
            Gate::active(),
        );
        processor.process(
            RawEvent::MouseUp {
                x: 2.0,
                y: 3.0,
                button: Button::Left,
            },
            2,
            Gate::active(),
        );
        processor.process(
            RawEvent::MouseDown {
                x: 4.0,
                y: 5.0,
                button: Button::Left,
            },
            3,
            Gate::active(),
        );
        processor.process(
            RawEvent::MouseDragged {
                x: 30.0,
                y: 40.0,
                button: Button::Left,
            },
            4,
            Gate::active(),
        );
        processor.process(
            RawEvent::MouseUp {
                x: 30.0,
                y: 40.0,
                button: Button::Left,
            },
            5,
            Gate::active(),
        );
        processor.process(
            RawEvent::Scroll {
                x: 8.0,
                y: 9.0,
                dx: 0.0,
                dy: 1.0,
            },
            6,
            Gate::active(),
        );
        processor.process(
            RawEvent::Scroll {
                x: 8.0,
                y: 9.0,
                dx: 0.0,
                dy: 2.0,
            },
            7,
            Gate::active(),
        );
        processor.flush(200);
        let events: Vec<_> = receiver.try_iter().collect();
        assert!(matches!(events[0], HumanEvent::Click { .. }));
        assert!(matches!(
            events[1],
            HumanEvent::Drag {
                from: (4.0, 5.0),
                to: (30.0, 40.0),
                ..
            }
        ));
        assert!(matches!(events[2], HumanEvent::Scroll { dy: 3.0, .. }));
    }

    #[test]
    fn secure_input_and_injected_events_are_suppressed() {
        let (mut processor, receiver, _) = harness(8);
        processor.process(
            RawEvent::KeyDown {
                key: Key::Named("Enter"),
                modifiers: Modifiers::default(),
            },
            1,
            Gate {
                secure_input: true,
                ..Gate::active()
            },
        );
        processor.process(
            RawEvent::KeyDown {
                key: Key::Named("Escape"),
                modifiers: Modifiers::default(),
            },
            2,
            Gate {
                injected: true,
                ..Gate::active()
            },
        );
        assert_eq!(receiver.try_iter().count(), 0);
    }

    #[test]
    fn bounded_queue_accounts_for_drops() {
        let (mut processor, _receiver, dropped) = harness(0);
        processor.process(
            RawEvent::KeyDown {
                key: Key::Named("Enter"),
                modifiers: Modifiers::default(),
            },
            1,
            Gate::active(),
        );
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn undrained_main_queue_times_out_without_blocking_worker_cleanup() {
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let (queued_tx, queued_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = main_sync_with(
                || 42,
                |job| queued_tx.send(job).unwrap(),
                std::time::Duration::from_millis(20),
            );
            finished_tx.send(result).unwrap();
        });

        let queued_job = queued_rx.recv().unwrap();
        let result = finished_rx
            .recv_timeout(std::time::Duration::from_millis(200))
            .expect("worker must not remain blocked on an undrained main queue");
        assert!(result.is_err());
        drop(queued_job);
        worker.join().unwrap();
    }

    #[test]
    fn timed_out_main_queue_cleanup_remains_deferred_without_blocking_join() {
        let cleaned = Arc::new(AtomicBool::new(false));
        let worker_cleaned = cleaned.clone();
        let (queued_tx, queued_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            main_sync_cleanup_with(
                move || worker_cleaned.store(true, Ordering::Release),
                |job| queued_tx.send(job).unwrap(),
                std::time::Duration::from_millis(20),
            )
        });

        let queued_job = queued_rx.recv().unwrap();
        assert!(worker.join().unwrap().is_err());
        assert!(!cleaned.load(Ordering::Acquire));
        queued_job();
        assert!(cleaned.load(Ordering::Acquire));
    }

    #[test]
    fn disabled_tap_notifications_latch_incomplete_capture() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut continuity = TapContinuity::default();

        assert!(continuity.observe_event(EVENT_TAP_DISABLED_BY_TIMEOUT, &dropped));
        assert!(continuity.observe_event(EVENT_TAP_DISABLED_BY_USER_INPUT, &dropped));
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert!(continuity.lost());
    }

    #[test]
    fn disabled_tap_backstop_latches_drop_even_without_notification() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut continuity = TapContinuity::default();

        continuity.observe_enabled_state(false, &dropped);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert!(continuity.lost());
    }

    #[test]
    fn disabled_tap_recovery_failure_terminates_capture_fail_closed() {
        let stop = AtomicBool::new(false);
        let reenable_attempted = AtomicBool::new(false);

        assert!(!recover_tap_with(
            &stop,
            || reenable_attempted.store(true, Ordering::Release),
            || false,
        ));
        assert!(reenable_attempted.load(Ordering::Acquire));
        assert!(stop.load(Ordering::Acquire));
    }

    #[test]
    fn rejects_pid_and_window_identifiers_that_cannot_round_trip() {
        for config in [
            CaptureConfig {
                pid: -1,
                window_id: 1,
            },
            CaptureConfig {
                pid: i64::from(i32::MAX) + 1,
                window_id: 1,
            },
            CaptureConfig {
                pid: 1,
                window_id: u64::from(u32::MAX) + 1,
            },
            CaptureConfig {
                pid: 1,
                window_id: 0,
            },
        ] {
            assert!(checked_identifiers(&config).is_err(), "accepted {config:?}");
        }
        assert_eq!(
            checked_identifiers(&CaptureConfig {
                pid: i64::from(i32::MAX),
                window_id: u64::from(u32::MAX),
            })
            .unwrap(),
            (i32::MAX, u32::MAX)
        );
    }

    #[test]
    fn orphan_mouse_up_is_suppressed_after_rejected_down_or_mid_press_start() {
        let (mut processor, receiver, _) = harness(8);
        let down = RawEvent::MouseDown {
            x: 1.0,
            y: 2.0,
            button: Button::Left,
        };
        let up = RawEvent::MouseUp {
            x: 3.0,
            y: 4.0,
            button: Button::Left,
        };

        processor.process(
            down,
            1,
            Gate {
                pointer_in_target: false,
                ..Gate::active()
            },
        );
        processor.process(up.clone(), 2, Gate::active());
        processor.process(up, 3, Gate::active());
        assert_eq!(receiver.try_iter().count(), 0);
    }

    #[test]
    fn worker_self_termination_keeps_run_loop_alive_until_owner_stop_and_drop() {
        use std::sync::Mutex;

        struct FakeRunLoop;

        impl RunLoopApi for FakeRunLoop {
            unsafe fn stop(run_loop: usize) {
                let events = &*(run_loop as *const Mutex<Vec<&'static str>>);
                events.lock().unwrap().push("stop");
            }

            unsafe fn release(run_loop: usize) {
                let events = Arc::from_raw(run_loop as *const Mutex<Vec<&'static str>>);
                events.lock().unwrap().push("release");
            }
        }

        for explicit_stop in [true, false] {
            let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
            let owner_run_loop = unsafe {
                RetainedRunLoop::<FakeRunLoop>::from_retained(Arc::into_raw(events.clone()) as usize)
            };
            let worker_run_loop = unsafe {
                RetainedRunLoop::<FakeRunLoop>::from_retained(Arc::into_raw(events.clone()) as usize)
            };
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let (terminated, observed_termination) = std::sync::mpsc::channel();
            let thread = std::thread::spawn(move || {
                worker_stop.store(true, Ordering::Release);
                drop(worker_run_loop);
                terminated.send(()).unwrap();
            });
            let mut lifecycle = CaptureLifecycle {
                stop,
                run_loop: owner_run_loop,
                thread: Some(thread),
            };

            observed_termination.recv().unwrap();
            assert_eq!(Arc::strong_count(&events), 2);
            if explicit_stop {
                lifecycle.shutdown();
            }
            drop(lifecycle);

            assert_eq!(Arc::strong_count(&events), 1);
            assert_eq!(*events.lock().unwrap(), ["release", "stop", "release"]);
        }
    }

    #[test]
    fn drag_that_leaves_target_does_not_return_as_false_click() {
        let (mut processor, receiver, _) = harness(8);
        processor.process(
            RawEvent::MouseDown {
                x: 1.0,
                y: 2.0,
                button: Button::Left,
            },
            1,
            Gate::active(),
        );
        processor.process(
            RawEvent::MouseDragged {
                x: 101.0,
                y: 102.0,
                button: Button::Left,
            },
            2,
            Gate {
                pointer_in_target: false,
                ..Gate::active()
            },
        );
        processor.process(
            RawEvent::MouseUp {
                x: 3.0,
                y: 4.0,
                button: Button::Left,
            },
            3,
            Gate::active(),
        );
        assert_eq!(receiver.try_iter().count(), 0);
    }
}
