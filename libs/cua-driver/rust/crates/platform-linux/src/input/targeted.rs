//! X11 targeted one-shot input with explicit delivery semantics.
//!
//! The Linux adapter for the cross-platform contract in
//! [`cua_driver_core::interactive_input`]; embedders (cua-spacesd) call it
//! instead of injecting input themselves.
//!
//! - **Foreground**: activate the target window (when there is one), confirm
//!   the point is not covered by another window, and inject through XTest into
//!   the focused root. The real pointer moves and focus may change; the report
//!   says so.
//! - **Background**: synthesize events addressed to one window with
//!   XSendEvent, without moving the pointer or changing focus. Without a
//!   window, background input is addressed to the window under the point
//!   (pointer) or the focused window (keyboard); AUTO without a window is a
//!   screen-space request and goes to the foreground.
//! - **Auto** picks background whenever a window can be addressed and its
//!   toolkit honours synthetic events, and foreground otherwise. Toolkits known
//!   to drop XSendEvent input ([`synthetic_input_ignored_by`]: GTK3/4,
//!   Chromium/Electron) get foreground XTest under AUTO when the caller allows
//!   escalation, and an explicit background request to one of them is refused
//!   with `WouldRequireActivation` rather than reported as delivered.
//!
//! Every path round-trips to the X server (`GetInputFocus`) before it reports
//! delivery, so the events were processed, not merely written to a socket.
//! Wayland sessions have no per-window targeting and are refused as
//! `Unsupported` here; use the libei-backed tools instead.

use std::time::{Duration, Instant};

use cua_driver_core::interactive_input::{
    resolve_delivery, DeliveryReport, DeliveryUsed, KeyOp, PointerOp, TargetedButton,
    TargetedDelivery, TargetedInputError,
};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

pub(crate) const XBUTTON_LEFT: u8 = 1;
pub(crate) const XBUTTON_MIDDLE: u8 = 2;
pub(crate) const XBUTTON_RIGHT: u8 = 3;
pub(crate) const XBUTTON_SCROLL_UP: u8 = 4;
pub(crate) const XBUTTON_SCROLL_DOWN: u8 = 5;
pub(crate) const XBUTTON_SCROLL_LEFT: u8 = 6;
pub(crate) const XBUTTON_SCROLL_RIGHT: u8 = 7;

const XK_BACKSPACE: u32 = 0xff08;
pub(crate) const XK_TAB: u32 = 0xff09;
pub(crate) const XK_RETURN: u32 = 0xff0d;
const XK_ESCAPE: u32 = 0xff1b;
const XK_DELETE: u32 = 0xffff;
const XK_HOME: u32 = 0xff50;
const XK_LEFT: u32 = 0xff51;
const XK_UP: u32 = 0xff52;
const XK_RIGHT: u32 = 0xff53;
const XK_DOWN: u32 = 0xff54;
const XK_PAGE_UP: u32 = 0xff55;
const XK_PAGE_DOWN: u32 = 0xff56;
const XK_END: u32 = 0xff57;
const XK_INSERT: u32 = 0xff63;
const XK_SPACE: u32 = 0x0020;
const XK_CAPS_LOCK: u32 = 0xffe5;
pub(crate) const XK_SHIFT_L: u32 = 0xffe1;
const XK_CONTROL_L: u32 = 0xffe3;
const XK_ALT_L: u32 = 0xffe9;
const XK_SUPER_L: u32 = 0xffeb;
const XK_F1: u32 = 0xffbe;

/// Hard bound on how long foreground delivery waits for the window manager
/// to restack an activated window before refusing an occluded point.
const OCCLUSION_SETTLE: Duration = Duration::from_millis(300);

/// One targeted pointer operation.
#[derive(Debug, Clone, PartialEq)]
pub struct PointerRequest {
    /// Target X window, or `None` to address the window under the point.
    pub window: Option<u32>,
    pub delivery: TargetedDelivery,
    /// Whether AUTO may escalate to foreground (moves the real pointer and
    /// may activate the window).
    pub allow_auto_foreground: bool,
    /// Root-window (screen) pixel point.
    pub x: i32,
    pub y: i32,
    pub op: PointerOp,
}

/// One targeted keyboard operation.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyRequest {
    /// Target X window, or `None` to address the focused window.
    pub window: Option<u32>,
    pub delivery: TargetedDelivery,
    pub allow_auto_foreground: bool,
    pub op: KeyOp,
}

pub(crate) fn failed(error: impl std::fmt::Display) -> TargetedInputError {
    TargetedInputError::DeliveryFailed(format!("X11 input delivery failed: {error}"))
}

pub(crate) fn gone() -> TargetedInputError {
    TargetedInputError::TargetUnavailable("window is gone".into())
}

/// The toolkit of process `pid` when it is one that drops synthetic
/// (XSendEvent) pointer and key events: GTK3/GTK4 read input through XInput2
/// and ignore core events with `send_event` set, and Chromium-based apps
/// filter synthetic events. `None`: unknown or a toolkit that accepts them
/// (Xt/Xaw, Tk, Qt5 core events, SDL, plain Xlib).
///
/// Detection reads `/proc/<pid>/maps` (bounded) for the toolkit's shared
/// library, and `/proc/<pid>/exe` for Chromium/Electron binaries.
pub fn synthetic_input_ignored_by(pid: u32) -> Option<&'static str> {
    use std::io::Read;
    const MAX_MAPS: u64 = 8 << 20;
    let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    if ["chrome", "chromium", "chromium-browser", "electron", "code"]
        .iter()
        .any(|name| exe == *name)
    {
        return Some("Chromium");
    }
    let mut maps = String::new();
    std::fs::File::open(format!("/proc/{pid}/maps"))
        .ok()?
        .take(MAX_MAPS)
        .read_to_string(&mut maps)
        .ok()?;
    toolkit_from_maps(&maps)
}

fn toolkit_from_maps(maps: &str) -> Option<&'static str> {
    if maps.contains("libgtk-4.so") {
        Some("GTK4")
    } else if maps.contains("libgtk-3.so") {
        Some("GTK3")
    } else if maps.contains("libcef.so") || maps.contains("libelectron") {
        Some("Chromium")
    } else {
        None
    }
}

/// Map the protocol key vocabulary (DOM-style and macOS names) onto X names.
pub fn x_key_name(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "arrowup" => "up".into(),
        "arrowdown" => "down".into(),
        "arrowleft" => "left".into(),
        "arrowright" => "right".into(),
        "command" | "cmd" => "super".into(),
        "option" => "alt".into(),
        _ => key.to_owned(),
    }
}

pub(crate) fn x_button(button: TargetedButton) -> u8 {
    match button {
        TargetedButton::Left => XBUTTON_LEFT,
        TargetedButton::Middle => XBUTTON_MIDDLE,
        TargetedButton::Right => XBUTTON_RIGHT,
        TargetedButton::Back => 8,
        TargetedButton::Forward => 9,
    }
}

// ---------------------------------------------------------------------------
// X11 facts
// ---------------------------------------------------------------------------

pub(crate) fn connect() -> Result<(RustConnection, Window), TargetedInputError> {
    if let Some(reason) = crate::wayland::wayland_input_unavailable_reason() {
        return Err(TargetedInputError::Unsupported(reason));
    }
    let (conn, screen) = RustConnection::connect(None).map_err(|error| {
        TargetedInputError::TargetUnavailable(format!(
            "cannot connect to the X server (DISPLAY={:?}): {error}",
            std::env::var("DISPLAY").ok()
        ))
    })?;
    let root = conn.setup().roots[screen].root;
    Ok((conn, root))
}

fn atom(conn: &RustConnection, name: &str) -> Option<Atom> {
    conn.intern_atom(false, name.as_bytes())
        .ok()?
        .reply()
        .ok()
        .map(|reply| reply.atom)
}

fn atoms32(conn: &RustConnection, window: Window, name: &str, kind: impl Into<Atom>) -> Vec<u32> {
    let Some(property) = atom(conn, name) else {
        return Vec::new();
    };
    conn.get_property(false, window, property, kind.into(), 0, 4096)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| reply.value32().map(|values| values.collect()))
        .unwrap_or_default()
}

pub(crate) fn window_pid(conn: &RustConnection, window: Window) -> Option<u32> {
    atoms32(conn, window, "_NET_WM_PID", AtomEnum::CARDINAL)
        .first()
        .copied()
}

/// Where untargeted key events go when no window is active: the focus
/// window, or with `PointerRoot` focus the top-level window under the
/// pointer. `None` when they would reach no window.
fn keyboard_destination(conn: &RustConnection, root: Window) -> Option<Window> {
    let focus = conn.get_input_focus().ok()?.reply().ok()?.focus;
    match focus {
        // `None`: key events are discarded.
        0 => None,
        // `PointerRoot`: the window under the pointer gets them.
        1 => conn
            .query_pointer(root)
            .ok()?
            .reply()
            .ok()
            .map(|p| p.child)
            .filter(|child| *child != x11rb::NONE),
        window if window == root => None,
        window => Some(window),
    }
}

pub(crate) fn active_window(conn: &RustConnection, root: Window) -> Option<Window> {
    atoms32(conn, root, "_NET_ACTIVE_WINDOW", AtomEnum::WINDOW)
        .first()
        .copied()
        .filter(|window| *window != x11rb::NONE)
        .or_else(|| {
            conn.get_input_focus()
                .ok()?
                .reply()
                .ok()
                .map(|focus| focus.focus)
                .filter(|window| *window > 1)
        })
}

fn stacking(conn: &RustConnection, root: Window) -> Vec<Window> {
    for name in ["_NET_CLIENT_LIST_STACKING", "_NET_CLIENT_LIST"] {
        let Some(property) = atom(conn, name) else {
            continue;
        };
        if let Some(reply) = conn
            .get_property(false, root, property, AtomEnum::WINDOW, 0, u32::MAX)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            if reply.type_ != x11rb::NONE {
                return reply
                    .value32()
                    .map(|values| values.collect())
                    .unwrap_or_default();
            }
        }
    }
    conn.query_tree(root)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|tree| tree.children)
        .unwrap_or_default()
}

/// The frontmost mapped top-level client window containing a root point.
/// Input-only, degenerate and drag-and-drop windows, and this process's own
/// overlay surfaces (the agent cursor), are skipped.
pub(crate) fn window_at(conn: &RustConnection, root: Window, x: i32, y: i32) -> Option<Window> {
    let own_pid = std::process::id();
    let dnd = atom(conn, "_NET_WM_WINDOW_TYPE_DND");
    for xid in stacking(conn, root).into_iter().rev() {
        let Some(attributes) = conn
            .get_window_attributes(xid)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            continue;
        };
        if attributes.map_state != MapState::VIEWABLE || attributes.class == WindowClass::INPUT_ONLY
        {
            continue;
        }
        let Some(geometry) = conn
            .get_geometry(xid)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            continue;
        };
        if geometry.width <= 2 || geometry.height <= 2 || window_pid(conn, xid) == Some(own_pid) {
            continue;
        }
        if dnd.is_some_and(|dnd| {
            atoms32(conn, xid, "_NET_WM_WINDOW_TYPE", AtomEnum::ATOM).contains(&dnd)
        }) {
            continue;
        }
        let (wx, wy) = conn
            .translate_coordinates(xid, root, 0, 0)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| (i32::from(reply.dst_x), i32::from(reply.dst_y)))
            .unwrap_or((i32::from(geometry.x), i32::from(geometry.y)));
        if x >= wx
            && y >= wy
            && x < wx + i32::from(geometry.width)
            && y < wy + i32::from(geometry.height)
        {
            return Some(xid);
        }
    }
    None
}

/// Whether `ancestor` is a parent (at any depth) of `window`. Bounded walk.
fn is_ancestor(conn: &RustConnection, ancestor: Window, window: Window) -> bool {
    let mut current = window;
    for _ in 0..16 {
        let Some(tree) = conn.query_tree(current).ok().and_then(|c| c.reply().ok()) else {
            return false;
        };
        if tree.parent == ancestor {
            return true;
        }
        if tree.parent == tree.root || tree.parent == x11rb::NONE {
            return false;
        }
        current = tree.parent;
    }
    false
}

/// Activate a window through the window manager (EWMH `_NET_ACTIVE_WINDOW`),
/// raise it and give it input focus, then round-trip.
pub(crate) fn activate(
    conn: &RustConnection,
    root: Window,
    window: Window,
) -> Result<(), TargetedInputError> {
    let _ = conn.map_window(window);
    if let Some(message_type) = atom(conn, "_NET_ACTIVE_WINDOW") {
        let event =
            ClientMessageEvent::new(32, window, message_type, [1, x11rb::CURRENT_TIME, 0, 0, 0]);
        conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )
        .map_err(failed)?;
    }
    let _ = conn.configure_window(
        window,
        &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
    );
    let _ = conn.set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME);
    sync(conn)
}

/// Round-trip so every request on this connection has been processed by the
/// server before delivery is reported (a bare flush is not enough: requests
/// can be lost with a short-lived connection).
pub(crate) fn sync(conn: &RustConnection) -> Result<(), TargetedInputError> {
    conn.get_input_focus()
        .map_err(failed)?
        .reply()
        .map_err(failed)?;
    Ok(())
}

fn to_local(
    conn: &RustConnection,
    root: Window,
    window: Window,
    x: i32,
    y: i32,
) -> Result<(i32, i32), TargetedInputError> {
    let reply = conn
        .translate_coordinates(root, window, x as i16, y as i16)
        .map_err(failed)?
        .reply()
        .map_err(|_| gone())?;
    Ok((i32::from(reply.dst_x), i32::from(reply.dst_y)))
}

/// Refuse, rather than report a delivery that went to another window, when
/// XTest input at a root point would land on a window other than `target`.
/// Waits (bounded) for the window manager to restack after activation.
pub(crate) fn ensure_point_hits(
    conn: &RustConnection,
    root: Window,
    target: Window,
    x: i32,
    y: i32,
) -> Result<(), TargetedInputError> {
    let deadline = Instant::now() + OCCLUSION_SETTLE;
    loop {
        let hit = window_at(conn, root, x, y);
        let covering = match hit {
            None => None,
            Some(hit)
                if hit == target
                    || is_ancestor(conn, hit, target)
                    || is_ancestor(conn, target, hit) =>
            {
                None
            }
            Some(hit) => Some(hit),
        };
        let Some(covering) = covering else {
            return Ok(());
        };
        if Instant::now() >= deadline {
            return Err(TargetedInputError::WouldRequireActivation(format!(
                "the point ({x}, {y}) is covered by window 0x{covering:x}; XTest input would \
                 reach it instead of the target 0x{target:x}"
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------
// Keyboard map and XTest primitives
// ---------------------------------------------------------------------------

pub(crate) fn modifier_keysym(name: &str) -> Option<u32> {
    Some(match name.to_ascii_lowercase().as_str() {
        "shift" => XK_SHIFT_L,
        "control" | "ctrl" => XK_CONTROL_L,
        "alt" | "option" => XK_ALT_L,
        "command" | "cmd" | "super" | "win" | "meta" => XK_SUPER_L,
        _ => return None,
    })
}

/// Resolve a key name into an X keysym. Named keys win; a single character
/// falls back to its character keysym; `keysym:0x..` passes through.
pub(crate) fn resolve_key_keysym(key: &str) -> Option<u32> {
    if let Some(hex) = key.strip_prefix("keysym:0x") {
        return u32::from_str_radix(hex, 16).ok();
    }
    if let Some(sym) = named_key_keysym(key) {
        return Some(sym);
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(single), None) => Some(char_keysym(single)),
        _ => None,
    }
}

fn named_key_keysym(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "enter" | "return" => XK_RETURN,
        "backspace" => XK_BACKSPACE,
        "delete" | "del" => XK_DELETE,
        "tab" => XK_TAB,
        "escape" | "esc" => XK_ESCAPE,
        "space" | "spacebar" => XK_SPACE,
        "up" => XK_UP,
        "down" => XK_DOWN,
        "left" => XK_LEFT,
        "right" => XK_RIGHT,
        "home" => XK_HOME,
        "end" => XK_END,
        "pageup" => XK_PAGE_UP,
        "pagedown" => XK_PAGE_DOWN,
        "insert" => XK_INSERT,
        "capslock" => XK_CAPS_LOCK,
        "shift" => XK_SHIFT_L,
        "control" | "ctrl" => XK_CONTROL_L,
        "alt" | "option" => XK_ALT_L,
        "command" | "cmd" | "super" | "win" | "meta" => XK_SUPER_L,
        _ => {
            if let Some(rest) = lower.strip_prefix('f') {
                if let Ok(number) = rest.parse::<u32>() {
                    if (1..=35).contains(&number) {
                        return Some(XK_F1 + number - 1);
                    }
                }
            }
            return None;
        }
    })
}

/// Latin-1 code points are keysyms directly; higher code points use the
/// Unicode keysym range (0x01000000 + code point).
pub(crate) fn char_keysym(character: char) -> u32 {
    let code = character as u32;
    if (0x20..=0xff).contains(&code) {
        code
    } else {
        0x0100_0000 + code
    }
}

/// A snapshot of the server's keysym-to-keycode mapping.
pub(crate) struct KeyboardMap {
    mapping: GetKeyboardMappingReply,
}

impl KeyboardMap {
    pub(crate) fn load(conn: &RustConnection) -> Result<Self, TargetedInputError> {
        // Keycodes 8..=255: the range `char_to_keycode_shift` and the
        // spare-keycode remap index from.
        let mapping = conn
            .get_keyboard_mapping(8, 248)
            .map_err(failed)?
            .reply()
            .map_err(failed)?;
        Ok(Self { mapping })
    }

    /// `(keycode, needs_shift)` producing `keysym` (column 0 or 1).
    pub(crate) fn lookup(&self, keysym: u32) -> Option<(u8, bool)> {
        super::char_to_keycode_shift(&self.mapping, keysym)
    }

    /// Like [`lookup`](Self::lookup), but borrows a spare keycode when the map
    /// has no keycode for `keysym` (non-Latin text on a US map, sparse
    /// headless keymaps). The guard restores the map on drop.
    pub(crate) fn lookup_or_remap<'a>(
        &self,
        conn: &'a RustConnection,
        keysym: u32,
    ) -> Result<(u8, bool, Option<super::RemappedKeycode<'a>>), TargetedInputError> {
        if let Some((code, shift)) = self.lookup(keysym) {
            return Ok((code, shift, None));
        }
        let guard = super::remap_spare_keycode(conn, &self.mapping, keysym).map_err(failed)?;
        Ok((guard.keycode, false, Some(guard)))
    }
}

pub(crate) fn fake_motion(
    conn: &RustConnection,
    root: Window,
    x: i32,
    y: i32,
) -> Result<(), TargetedInputError> {
    let x = x.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
    let y = y.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
    conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 0, 0, root, x, y, 0)
        .map_err(failed)?;
    Ok(())
}

pub(crate) fn fake_button(
    conn: &RustConnection,
    press: bool,
    button: u8,
    root: Window,
) -> Result<(), TargetedInputError> {
    let kind = if press {
        BUTTON_PRESS_EVENT
    } else {
        BUTTON_RELEASE_EVENT
    };
    conn.xtest_fake_input(kind, button, 0, root, 0, 0, 0)
        .map_err(failed)?;
    Ok(())
}

pub(crate) fn fake_key(
    conn: &RustConnection,
    press: bool,
    keycode: u8,
    root: Window,
) -> Result<(), TargetedInputError> {
    let kind = if press {
        KEY_PRESS_EVENT
    } else {
        KEY_RELEASE_EVENT
    };
    conn.xtest_fake_input(kind, keycode, 0, root, 0, 0, 0)
        .map_err(failed)?;
    Ok(())
}

pub(crate) fn press_modifiers(
    conn: &RustConnection,
    root: Window,
    keymap: &KeyboardMap,
    modifiers: &[String],
) -> Result<Vec<u8>, TargetedInputError> {
    let mut held = Vec::new();
    for name in modifiers {
        if let Some((code, _)) = modifier_keysym(name).and_then(|sym| keymap.lookup(sym)) {
            fake_key(conn, true, code, root)?;
            held.push(code);
        }
    }
    Ok(held)
}

pub(crate) fn release_keys(
    conn: &RustConnection,
    root: Window,
    held: &[u8],
) -> Result<(), TargetedInputError> {
    for code in held.iter().rev() {
        fake_key(conn, false, *code, root)?;
    }
    Ok(())
}

pub(crate) fn key_code(keymap: &KeyboardMap, key: &str) -> Result<(u8, bool), TargetedInputError> {
    let sym = resolve_key_keysym(key)
        .or_else(|| modifier_keysym(key))
        .ok_or_else(|| TargetedInputError::Unsupported(format!("unsupported key name {key}")))?;
    keymap
        .lookup(sym)
        .ok_or_else(|| failed(format!("no keycode maps to {key}")))
}

pub(crate) fn tap(
    conn: &RustConnection,
    root: Window,
    keymap: &KeyboardMap,
    code: u8,
    shift: bool,
) -> Result<(), TargetedInputError> {
    let shift_code = if shift {
        keymap.lookup(XK_SHIFT_L).map(|(code, _)| code)
    } else {
        None
    };
    if let Some(shift) = shift_code {
        fake_key(conn, true, shift, root)?;
    }
    fake_key(conn, true, code, root)?;
    fake_key(conn, false, code, root)?;
    if let Some(shift) = shift_code {
        fake_key(conn, false, shift, root)?;
    }
    Ok(())
}

/// Commit composed text through XTest into the focused window. Characters
/// missing from the keyboard map borrow a spare keycode, so non-Latin text is
/// committed rather than silently skipped.
pub(crate) fn type_xtest(
    conn: &RustConnection,
    root: Window,
    keymap: &KeyboardMap,
    text: &str,
) -> Result<(), TargetedInputError> {
    for character in text.chars() {
        let sym = match character {
            '\n' | '\r' => XK_RETURN,
            '\t' => XK_TAB,
            other => char_keysym(other),
        };
        let (code, shift, guard) = keymap.lookup_or_remap(conn, sym)?;
        tap(conn, root, keymap, code, shift)?;
        if guard.is_some() {
            // Let the focused client translate the event under the temporary
            // mapping before the guard restores it.
            sync(conn)?;
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(guard);
    }
    Ok(())
}

pub(crate) fn background_key_event(
    conn: &RustConnection,
    root: Window,
    window: Window,
    keycode: u8,
    press: bool,
) -> Result<(), TargetedInputError> {
    let event = KeyPressEvent {
        response_type: if press {
            KEY_PRESS_EVENT
        } else {
            KEY_RELEASE_EVENT
        },
        detail: keycode,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        root,
        event: window,
        child: x11rb::NONE,
        root_x: 0,
        root_y: 0,
        event_x: 0,
        event_y: 0,
        state: KeyButMask::from(0u16),
        same_screen: true,
    };
    let mask = if press {
        EventMask::KEY_PRESS
    } else {
        EventMask::KEY_RELEASE
    };
    conn.send_event(true, window, mask, event).map_err(failed)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

/// Deliver one pointer operation.
pub fn pointer(request: &PointerRequest) -> Result<DeliveryReport, TargetedInputError> {
    let (conn, root) = connect()?;
    // Background input goes to the requested window, else (explicit
    // BACKGROUND only) the one under the point; its toolkit decides whether
    // synthetic events can reach it. AUTO without a window is a screen-space
    // request and uses the real pointer, like the macOS and Windows adapters.
    let target = match request.window {
        Some(window) => Some(window),
        None if request.delivery == TargetedDelivery::Background => {
            window_at(&conn, root, request.x, request.y)
        }
        None => None,
    };
    let (used, note) = resolve_delivery(
        request.delivery,
        target.is_some(),
        target
            .and_then(|window| window_pid(&conn, window))
            .and_then(synthetic_input_ignored_by),
        request.allow_auto_foreground,
        "pointer events",
    )?;
    let mut report = match used {
        DeliveryUsed::Background => {
            let target = target.ok_or_else(|| {
                TargetedInputError::WouldRequireActivation(
                    "no window under the point to address background input to".into(),
                )
            })?;
            pointer_background(&conn, root, target, request.x, request.y, &request.op)?
        }
        DeliveryUsed::Foreground => pointer_foreground(
            &conn,
            root,
            request.window,
            request.x,
            request.y,
            &request.op,
        )?,
    };
    report.note = note;
    Ok(report)
}

fn pointer_background(
    conn: &RustConnection,
    root: Window,
    target: Window,
    x: i32,
    y: i32,
    op: &PointerOp,
) -> Result<DeliveryReport, TargetedInputError> {
    use super as input;
    let (lx, ly) = to_local(conn, root, target, x, y)?;
    let xid = u64::from(target);
    let result = match op {
        PointerOp::Click {
            button,
            count,
            modifiers,
        } => {
            let modifiers: Vec<&str> = modifiers.iter().map(String::as_str).collect();
            input::send_click_with_modifiers(
                xid,
                lx,
                ly,
                (*count).max(1) as usize,
                x_button(*button),
                &modifiers,
            )
        }
        PointerOp::Move => input::send_motion(xid, lx, ly, None),
        PointerOp::Down { button } => input::send_button_down(xid, lx, ly, x_button(*button)),
        PointerOp::Up { button } => input::send_button_up(xid, lx, ly, x_button(*button)),
        PointerOp::Drag { path, button, .. } => {
            let button = x_button(*button);
            let mut result = input::send_button_down(xid, lx, ly, button);
            let mut last = (lx, ly);
            for (px, py) in path {
                if result.is_err() {
                    break;
                }
                last = to_local(conn, root, target, *px, *py)?;
                result = input::send_motion(xid, last.0, last.1, Some(button));
            }
            result.and_then(|_| input::send_button_up(xid, last.0, last.1, button))
        }
        PointerOp::Scroll { dx, dy } => {
            let mut result = Ok(());
            for (amount, negative, positive) in [
                (*dy, XBUTTON_SCROLL_UP, XBUTTON_SCROLL_DOWN),
                (*dx, XBUTTON_SCROLL_LEFT, XBUTTON_SCROLL_RIGHT),
            ] {
                if amount != 0 && result.is_ok() {
                    let button = if amount < 0 { negative } else { positive };
                    result = input::send_click(xid, lx, ly, amount.unsigned_abs() as usize, button);
                }
            }
            result
        }
    };
    result.map_err(failed)?;
    Ok(DeliveryReport {
        delivery: DeliveryUsed::Background,
        focus_changed: false,
        pointer_moved: false,
        detail: "x11 XSendEvent to the target window".into(),
        note: None,
    })
}

fn pointer_foreground(
    conn: &RustConnection,
    root: Window,
    window: Option<Window>,
    x: i32,
    y: i32,
    op: &PointerOp,
) -> Result<DeliveryReport, TargetedInputError> {
    let before = active_window(conn, root);
    if let Some(window) = window {
        if before != Some(window) {
            activate(conn, root, window)?;
        }
        if !matches!(op, PointerOp::Move) {
            ensure_point_hits(conn, root, window, x, y)?;
        }
    }
    let keymap = KeyboardMap::load(conn)?;
    fake_motion(conn, root, x, y)?;
    match op {
        PointerOp::Click {
            button,
            count,
            modifiers,
        } => {
            let held = press_modifiers(conn, root, &keymap, modifiers)?;
            let button = x_button(*button);
            for index in 0..(*count).max(1) {
                fake_button(conn, true, button, root)?;
                fake_button(conn, false, button, root)?;
                if index + 1 < (*count).max(1) {
                    // Chromium needs the first pair processed before the next
                    // for a DOM dblclick.
                    sync(conn)?;
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            release_keys(conn, root, &held)?;
        }
        PointerOp::Move => {}
        PointerOp::Down { button } => fake_button(conn, true, x_button(*button), root)?,
        PointerOp::Up { button } => fake_button(conn, false, x_button(*button), root)?,
        PointerOp::Drag {
            path,
            button,
            modifiers,
        } => {
            let held = press_modifiers(conn, root, &keymap, modifiers)?;
            let button = x_button(*button);
            fake_button(conn, true, button, root)?;
            for (px, py) in path {
                fake_motion(conn, root, *px, *py)?;
                sync(conn)?;
                std::thread::sleep(Duration::from_millis(4));
            }
            fake_button(conn, false, button, root)?;
            release_keys(conn, root, &held)?;
        }
        PointerOp::Scroll { dx, dy } => {
            for (amount, negative, positive) in [
                (*dy, XBUTTON_SCROLL_UP, XBUTTON_SCROLL_DOWN),
                (*dx, XBUTTON_SCROLL_LEFT, XBUTTON_SCROLL_RIGHT),
            ] {
                let button = if amount < 0 { negative } else { positive };
                for _ in 0..amount.unsigned_abs() {
                    fake_button(conn, true, button, root)?;
                    fake_button(conn, false, button, root)?;
                }
            }
        }
    }
    sync(conn)?;
    Ok(DeliveryReport {
        delivery: DeliveryUsed::Foreground,
        focus_changed: before != active_window(conn, root),
        pointer_moved: true,
        detail: "x11 XTest into the focused root".into(),
        note: None,
    })
}

/// Deliver one keyboard operation.
pub fn keyboard(request: &KeyRequest) -> Result<DeliveryReport, TargetedInputError> {
    let (conn, root) = connect()?;
    // Background keys go to the requested window, else the focused one.
    let target = match request.window {
        Some(window) => Some(window),
        None if request.delivery != TargetedDelivery::Foreground => active_window(&conn, root),
        None => None,
    };
    let (used, note) = resolve_delivery(
        request.delivery,
        request.window.is_some(),
        target
            .and_then(|window| window_pid(&conn, window))
            .and_then(synthetic_input_ignored_by),
        request.allow_auto_foreground,
        "key events",
    )?;
    let mut report = match used {
        DeliveryUsed::Background => {
            let target = target.ok_or_else(|| {
                TargetedInputError::WouldRequireActivation(
                    "no window to address background keyboard input to".into(),
                )
            })?;
            keyboard_background(&conn, root, target, &request.op)?
        }
        DeliveryUsed::Foreground => keyboard_foreground(&conn, root, request.window, &request.op)?,
    };
    report.note = note;
    Ok(report)
}

fn keyboard_background(
    conn: &RustConnection,
    root: Window,
    target: Window,
    op: &KeyOp,
) -> Result<DeliveryReport, TargetedInputError> {
    use super as input;
    let xid = u64::from(target);
    match op {
        KeyOp::Type(text) => input::send_type_text(xid, text).map_err(failed)?,
        KeyOp::Press {
            key,
            modifiers,
            repeat,
        } => {
            let modifiers: Vec<&str> = modifiers.iter().map(String::as_str).collect();
            for _ in 0..(*repeat).max(1) {
                input::send_key(xid, &x_key_name(key), &modifiers).map_err(failed)?;
            }
        }
        KeyOp::Hotkey(keys) => {
            let (last, modifiers) = keys
                .split_last()
                .ok_or_else(|| TargetedInputError::Unsupported("empty hotkey".into()))?;
            let modifiers: Vec<&str> = modifiers.iter().map(String::as_str).collect();
            input::send_key(xid, &x_key_name(last), &modifiers).map_err(failed)?;
        }
        KeyOp::Down(key) | KeyOp::Up(key) => {
            let keymap = KeyboardMap::load(conn)?;
            let (code, _) = key_code(&keymap, &x_key_name(key))?;
            background_key_event(conn, root, target, code, matches!(op, KeyOp::Down(_)))?;
            sync(conn)?;
        }
    }
    Ok(DeliveryReport {
        delivery: DeliveryUsed::Background,
        focus_changed: false,
        pointer_moved: false,
        detail: "x11 XSendEvent key events to the target window".into(),
        note: None,
    })
}

fn keyboard_foreground(
    conn: &RustConnection,
    root: Window,
    window: Option<Window>,
    op: &KeyOp,
) -> Result<DeliveryReport, TargetedInputError> {
    let before = active_window(conn, root);
    if let Some(window) = window {
        if before != Some(window) {
            activate(conn, root, window)?;
        }
    } else if before.is_none() && keyboard_destination(conn, root).is_none() {
        // XTest would report success while the keys reach nothing.
        return Err(TargetedInputError::TargetUnavailable(
            "no window has keyboard focus, so the keys would reach nothing; \
             click a window or target one first"
                .into(),
        ));
    }
    let keymap = KeyboardMap::load(conn)?;
    match op {
        KeyOp::Type(text) => type_xtest(conn, root, &keymap, text)?,
        KeyOp::Press {
            key,
            modifiers,
            repeat,
        } => {
            let held = press_modifiers(conn, root, &keymap, modifiers)?;
            let (code, shift) = key_code(&keymap, &x_key_name(key))?;
            for _ in 0..(*repeat).max(1) {
                tap(conn, root, &keymap, code, shift)?;
            }
            release_keys(conn, root, &held)?;
        }
        KeyOp::Hotkey(keys) => {
            let (last, modifiers) = keys
                .split_last()
                .ok_or_else(|| TargetedInputError::Unsupported("empty hotkey".into()))?;
            let held = press_modifiers(conn, root, &keymap, modifiers)?;
            let (code, shift) = key_code(&keymap, &x_key_name(last))?;
            tap(conn, root, &keymap, code, shift)?;
            release_keys(conn, root, &held)?;
        }
        KeyOp::Down(key) => {
            let (code, _) = key_code(&keymap, &x_key_name(key))?;
            fake_key(conn, true, code, root)?;
        }
        KeyOp::Up(key) => {
            let (code, _) = key_code(&keymap, &x_key_name(key))?;
            fake_key(conn, false, code, root)?;
        }
    }
    sync(conn)?;
    Ok(DeliveryReport {
        delivery: DeliveryUsed::Foreground,
        focus_changed: before != active_window(conn, root),
        pointer_moved: false,
        detail: "x11 XTest into the focused window".into(),
        note: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolkits_that_drop_synthetic_events_are_recognised() {
        let gtk3 = "7f00-7f10 r-xp 0 08:01 1 /usr/lib/aarch64-linux-gnu/libgtk-3.so.0.2409.32\n";
        assert_eq!(toolkit_from_maps(gtk3), Some("GTK3"));
        assert_eq!(
            toolkit_from_maps("x /usr/lib/libgtk-4.so.1\n"),
            Some("GTK4")
        );
        assert_eq!(
            toolkit_from_maps("x /opt/app/libcef.so\n"),
            Some("Chromium")
        );
        assert_eq!(
            toolkit_from_maps("x /usr/lib/libXaw.so.7\nx /usr/lib/libX11.so.6\n"),
            None
        );
        // This test process is not a GTK app.
        assert_eq!(synthetic_input_ignored_by(std::process::id()), None);
    }

    #[test]
    fn protocol_key_names_map_to_x_names() {
        assert_eq!(x_key_name("ArrowUp"), "up");
        assert_eq!(x_key_name("cmd"), "super");
        assert_eq!(x_key_name("option"), "alt");
        assert_eq!(x_key_name("a"), "a");
    }

    #[test]
    fn buttons_map_to_x_core_buttons() {
        assert_eq!(x_button(TargetedButton::Left), XBUTTON_LEFT);
        assert_eq!(x_button(TargetedButton::Right), XBUTTON_RIGHT);
        assert_eq!(x_button(TargetedButton::Middle), XBUTTON_MIDDLE);
        assert_eq!(x_button(TargetedButton::Back), 8);
        assert_eq!(x_button(TargetedButton::Forward), 9);
    }

    #[test]
    fn named_keys_resolve_to_expected_keysyms() {
        assert_eq!(named_key_keysym("enter"), Some(XK_RETURN));
        assert_eq!(named_key_keysym("Return"), Some(XK_RETURN));
        assert_eq!(named_key_keysym("escape"), Some(XK_ESCAPE));
        assert_eq!(named_key_keysym("pagedown"), Some(XK_PAGE_DOWN));
        assert_eq!(named_key_keysym("f5"), Some(XK_F1 + 4));
        assert_eq!(named_key_keysym("nope"), None);
        assert_eq!(resolve_key_keysym("keysym:0xff0d"), Some(XK_RETURN));
    }

    #[test]
    fn single_character_keys_fall_back_to_char_keysym() {
        assert_eq!(resolve_key_keysym("a"), Some(u32::from(b'a')));
        assert_eq!(char_keysym('A'), u32::from(b'A'));
        assert_eq!(char_keysym(' '), XK_SPACE);
        assert_eq!(char_keysym('€'), 0x0100_0000 + u32::from('€'));
        assert_eq!(resolve_key_keysym("abc"), None);
    }

    #[test]
    fn modifier_names_map_to_modifier_keysyms() {
        assert_eq!(modifier_keysym("ctrl"), Some(XK_CONTROL_L));
        assert_eq!(modifier_keysym("control"), Some(XK_CONTROL_L));
        assert_eq!(modifier_keysym("alt"), Some(XK_ALT_L));
        assert_eq!(modifier_keysym("option"), Some(XK_ALT_L));
        assert_eq!(modifier_keysym("cmd"), Some(XK_SUPER_L));
        assert_eq!(modifier_keysym("shift"), Some(XK_SHIFT_L));
        assert_eq!(modifier_keysym("hyper"), None);
    }
}
