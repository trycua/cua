//! Background input injection for Linux.
//!
//! Pointer events (click, drag) go out via XSendEvent: synthetic events
//! delivered directly to a window without changing input focus — the Linux
//! equivalent of PostMessage on Windows.
//!
//! Keyboard input (type / key) goes through the XTEST extension instead.
//! XSendEvent keystrokes carry the `send_event` flag, which a large class of
//! apps deliberately ignore — terminal emulators (xterm and friends) most
//! notably, but also some games and security-sensitive apps. XTEST injects at
//! the server level with no such flag, so it lands everywhere a real keyboard
//! would. The one catch is that XTEST delivers to the *focused* window, so we
//! briefly focus the target, inject, and restore the prior focus (see
//! `with_focus`) to keep the same no-focus-steal contract as the pointer path.

use anyhow::Result;
use std::thread::sleep;
use std::time::Duration;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::*;
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

const CLICK_DELAY_MS: u64 = 35;
const KEY_DELAY_MS: u64 = 10;

fn xtest_available(conn: &RustConnection) -> bool {
    conn.extension_information(x11rb::protocol::xtest::X11_EXTENSION_NAME)
        .ok()
        .flatten()
        .is_some()
}

fn xtest_key_press(conn: &RustConnection, root: Window, keycode: u8) -> Result<()> {
    conn.xtest_fake_input(KEY_PRESS_EVENT, keycode, x11rb::CURRENT_TIME, root, 0, 0, 0)?;
    conn.flush()?;
    Ok(())
}

fn xtest_key_release(conn: &RustConnection, root: Window, keycode: u8) -> Result<()> {
    conn.xtest_fake_input(KEY_RELEASE_EVENT, keycode, x11rb::CURRENT_TIME, root, 0, 0, 0)?;
    conn.flush()?;
    Ok(())
}

fn xtest_settle(conn: &RustConnection) -> Result<()> {
    // Force a round-trip so the server processes the synthetic release before
    // this short-lived connection is dropped or the next tool call starts.
    conn.get_input_focus()?.reply()?;
    Ok(())
}

fn xtest_key_tap(conn: &RustConnection, root: Window, keycode: u8) -> Result<()> {
    xtest_key_press(conn, root, keycode)?;
    sleep(Duration::from_millis(KEY_DELAY_MS));
    xtest_key_release(conn, root, keycode)?;
    sleep(Duration::from_millis(KEY_DELAY_MS));
    Ok(())
}

/// Temporarily move X input focus to `window`, run `f`, then restore the
/// previously-focused window. XTEST delivers synthetic key events to whatever
/// window currently holds input focus, so we focus the target just long enough
/// to inject — and put focus back afterwards so the keyboard path honours the
/// same no-focus-steal contract the XSendEvent pointer path keeps.
fn with_focus<R>(
    conn: &RustConnection,
    window: Window,
    f: impl FnOnce(&RustConnection) -> Result<R>,
) -> Result<R> {
    let prev = conn.get_input_focus()?.reply()?;
    conn.set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)?;
    conn.flush()?;
    let result = f(conn);
    // Always attempt to restore the prior focus, even if injection failed.
    let _ = conn.set_input_focus(prev.revert_to, prev.focus, x11rb::CURRENT_TIME);
    let _ = conn.flush();
    result
}

/// Look up the keycode that produces `keysym`, plus whether Shift must be held
/// (i.e. the keysym lives in the shifted column of the keyboard map). Prefers
/// the unshifted column when a keysym appears in both.
fn keysym_to_keycode_shift(mapping: &GetKeyboardMappingReply, keysym: u32) -> Option<(u8, bool)> {
    let per = mapping.keysyms_per_keycode as usize;
    if per == 0 {
        return None;
    }
    for (i, syms) in mapping.keysyms.chunks(per).enumerate() {
        if syms.first() == Some(&keysym) {
            return Some(((8 + i) as u8, false));
        }
        if per > 1 && syms.get(1) == Some(&keysym) {
            return Some(((8 + i) as u8, true));
        }
    }
    None
}

/// Resolve a keysym to its keycode, ignoring the shift level.
fn keysym_to_keycode(mapping: &GetKeyboardMappingReply, keysym: u32) -> Option<u8> {
    keysym_to_keycode_shift(mapping, keysym).map(|(kc, _)| kc)
}

/// Send a button click (down + up) to a window at window-local coordinates.
pub fn send_click(xid: u64, x: i32, y: i32, count: usize, button: u8) -> Result<()> {
    let (conn, _) = RustConnection::connect(None)?;
    let window = xid as u32;

    // Get the root window for the display.
    let root = conn.setup().roots[0].root;

    for _ in 0..count {
        let press = ButtonPressEvent {
            response_type: BUTTON_PRESS_EVENT,
            detail: button,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: window,
            child: x11rb::NONE,
            root_x: 0, root_y: 0,
            event_x: x as i16,
            event_y: y as i16,
            state: KeyButMask::from(0u16),
            same_screen: true,
        };

        let release = ButtonReleaseEvent {
            response_type: BUTTON_RELEASE_EVENT,
            detail: button,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: window,
            child: x11rb::NONE,
            root_x: 0, root_y: 0,
            event_x: x as i16,
            event_y: y as i16,
            state: KeyButMask::from(0u16),
            same_screen: true,
        };

        conn.send_event(false, window, EventMask::BUTTON_PRESS, &press)?;
        sleep(Duration::from_millis(CLICK_DELAY_MS));
        conn.send_event(false, window, EventMask::BUTTON_RELEASE, &release)?;
        conn.flush()?;

        if count > 1 {
            sleep(Duration::from_millis(80));
        }
    }
    Ok(())
}

/// Send a press-drag-release gesture via XSendEvent (ButtonPress + MotionNotify steps + ButtonRelease).
///
/// `xid` — target window XID. `from_x/y`, `to_x/y` — window-local coords.
/// `duration_ms` — total budget. `steps` — interpolated MotionNotify events.
/// `button` — X11 button number (1=left, 2=middle, 3=right).
pub fn send_drag(
    xid: u64,
    from_x: i32,
    from_y: i32,
    to_x: i32,
    to_y: i32,
    duration_ms: u64,
    steps: usize,
    button: u8,
) -> Result<()> {
    let (conn, _) = RustConnection::connect(None)?;
    let window = xid as u32;
    let root = conn.setup().roots[0].root;
    let steps = steps.max(1);
    let step_delay_ms = if steps > 1 { duration_ms / steps as u64 } else { duration_ms };

    // ButtonPress at start.
    let press = ButtonPressEvent {
        response_type: BUTTON_PRESS_EVENT,
        detail: button,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        root, event: window, child: x11rb::NONE,
        root_x: 0, root_y: 0,
        event_x: from_x as i16, event_y: from_y as i16,
        state: KeyButMask::from(0u16),
        same_screen: true,
    };
    conn.send_event(false, window, EventMask::BUTTON_PRESS, &press)?;
    conn.flush()?;
    sleep(Duration::from_millis(CLICK_DELAY_MS));

    // Interpolated MotionNotify steps.
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let ix = from_x + ((to_x - from_x) as f64 * t).round() as i32;
        let iy = from_y + ((to_y - from_y) as f64 * t).round() as i32;
        let motion = MotionNotifyEvent {
            response_type: MOTION_NOTIFY_EVENT,
            detail: Motion::NORMAL,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root, event: window, child: x11rb::NONE,
            root_x: 0, root_y: 0,
            event_x: ix as i16, event_y: iy as i16,
            state: KeyButMask::from(0u16),
            same_screen: true,
        };
        conn.send_event(false, window, EventMask::POINTER_MOTION, &motion)?;
        conn.flush()?;
        if step_delay_ms > 0 {
            sleep(Duration::from_millis(step_delay_ms));
        }
    }

    // ButtonRelease at end.
    let release = ButtonReleaseEvent {
        response_type: BUTTON_RELEASE_EVENT,
        detail: button,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        root, event: window, child: x11rb::NONE,
        root_x: 0, root_y: 0,
        event_x: to_x as i16, event_y: to_y as i16,
        state: KeyButMask::from(0u16),
        same_screen: true,
    };
    conn.send_event(false, window, EventMask::BUTTON_RELEASE, &release)?;
    conn.flush()?;
    Ok(())
}

/// Type a string by faking key taps through XTEST. Works on any window that
/// can hold focus — including terminal emulators that ignore the `send_event`
/// flag and so never saw our old XSendEvent keystrokes.
pub fn send_type_text(xid: u64, text: &str) -> Result<()> {
    send_type_text_with_delay(xid, text, 0)
}

/// Type a string with an additional `inter_char_ms` delay between each character.
pub fn send_type_text_with_delay(xid: u64, text: &str, inter_char_ms: u64) -> Result<()> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    if !xtest_available(&conn) {
        anyhow::bail!("XTEST extension unavailable; cannot inject keyboard input");
    }
    let root = conn.setup().roots[screen_num].root;
    let mapping = conn.get_keyboard_mapping(8, 248)?.reply()?;
    let shift_kc = keysym_to_keycode(&mapping, 0xFFE1); // Shift_L

    with_focus(&conn, xid as Window, |conn| {
        for ch in text.chars() {
            // Keysym for ASCII / Latin-1 is just the codepoint.
            let Some((keycode, needs_shift)) = keysym_to_keycode_shift(&mapping, ch as u32) else {
                continue;
            };
            if needs_shift {
                if let Some(s) = shift_kc {
                    xtest_key_press(conn, root, s)?;
                }
            }
            xtest_key_tap(conn, root, keycode)?;
            if needs_shift {
                if let Some(s) = shift_kc {
                    xtest_key_release(conn, root, s)?;
                }
            }
            if inter_char_ms > 0 {
                sleep(Duration::from_millis(inter_char_ms));
            }
        }
        xtest_settle(conn)
    })
}

/// Send a named key (optionally with modifiers held) through XTEST.
pub fn send_key(xid: u64, key: &str, modifiers: &[&str]) -> Result<()> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    if !xtest_available(&conn) {
        anyhow::bail!("XTEST extension unavailable; cannot inject keyboard input");
    }
    let root = conn.setup().roots[screen_num].root;
    let mapping = conn.get_keyboard_mapping(8, 248)?.reply()?;

    let keysym = key_name_to_keysym(key)?;
    let keycode = keysym_to_keycode(&mapping, keysym)
        .ok_or_else(|| anyhow::anyhow!("Keysym 0x{keysym:X} not in keyboard map for key '{key}'"))?;
    let mod_keycodes: Vec<u8> = modifiers
        .iter()
        .filter_map(|m| modifier_keysym(m))
        .filter_map(|ks| keysym_to_keycode(&mapping, ks))
        .collect();

    with_focus(&conn, xid as Window, |conn| {
        for &mk in &mod_keycodes {
            xtest_key_press(conn, root, mk)?;
        }
        xtest_key_tap(conn, root, keycode)?;
        for &mk in mod_keycodes.iter().rev() {
            xtest_key_release(conn, root, mk)?;
        }
        xtest_settle(conn)
    })
}

/// Map a key name (e.g. "enter", "tab", "a") to its X11 keysym.
fn key_name_to_keysym(key: &str) -> Result<u32> {
    let keysym: u32 = match key.to_lowercase().as_str() {
        "return" | "enter" => 0xFF0D,
        "tab" => 0xFF09,
        "escape" | "esc" => 0xFF1B,
        "space" | " " => 0x0020,
        "backspace" => 0xFF08,
        "delete" | "del" => 0xFFFF,
        "insert" | "ins" => 0xFF63,
        "home" => 0xFF50,
        "end" => 0xFF57,
        "pageup" | "pgup" => 0xFF55,
        "pagedown" | "pgdn" => 0xFF56,
        "up" => 0xFF52,
        "down" => 0xFF54,
        "left" => 0xFF51,
        "right" => 0xFF53,
        "f1" => 0xFFBE, "f2" => 0xFFBF, "f3" => 0xFFC0, "f4" => 0xFFC1,
        "f5" => 0xFFC2, "f6" => 0xFFC3, "f7" => 0xFFC4, "f8" => 0xFFC5,
        "f9" => 0xFFC6, "f10" => 0xFFC7, "f11" => 0xFFC8, "f12" => 0xFFC9,
        "shift" => 0xFFE1, "ctrl" | "control" => 0xFFE3, "alt" => 0xFFE9,
        "super" | "meta" | "win" => 0xFFEB,
        "capslock" => 0xFFE5, "numlock" => 0xFF7F,
        s if s.chars().count() == 1 => s.chars().next().unwrap() as u32,
        _ => anyhow::bail!("Unknown key: {key}"),
    };
    Ok(keysym)
}

/// Map a modifier name to its keysym (Shift_L / Control_L / Alt_L / Super_L).
fn modifier_keysym(m: &str) -> Option<u32> {
    match m.to_lowercase().as_str() {
        "shift" => Some(0xFFE1),
        "ctrl" | "control" => Some(0xFFE3),
        "alt" | "mod1" => Some(0xFFE9),
        "super" | "mod4" | "win" | "meta" => Some(0xFFEB),
        _ => None,
    }
}
