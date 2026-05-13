//! Background input injection for Linux via X11 XSendEvent.
//!
//! XSendEvent sends synthetic events directly to a window without changing
//! input focus. This is the Linux equivalent of PostMessage on Windows.
//!
//! Note: Some apps check the `send_event` flag and ignore synthetic events
//! (e.g., some games, some security-sensitive apps). For those, the XTest
//! extension (XTestFakeKeyEvent) is the alternative, but it DOES send to
//! the focused window.

use anyhow::Result;
use std::thread::sleep;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

const CLICK_DELAY_MS: u64 = 35;
const KEY_DELAY_MS: u64 = 10;

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

/// Type a string by sending KeyPress/KeyRelease events for each character.
pub fn send_type_text(xid: u64, text: &str) -> Result<()> {
    let (conn, _) = RustConnection::connect(None)?;
    let window = xid as u32;
    let root = conn.setup().roots[0].root;

    for ch in text.chars() {
        let keycode = char_to_keycode(&conn, ch).unwrap_or(0);
        if keycode == 0 { continue; }

        let press = KeyPressEvent {
            response_type: KEY_PRESS_EVENT,
            detail: keycode,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: window,
            child: x11rb::NONE,
            root_x: 0, root_y: 0,
            event_x: 0, event_y: 0,
            state: KeyButMask::from(0u16),
            same_screen: true,
        };

        let release = KeyReleaseEvent {
            response_type: KEY_RELEASE_EVENT,
            detail: keycode,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: window,
            child: x11rb::NONE,
            root_x: 0, root_y: 0,
            event_x: 0, event_y: 0,
            state: KeyButMask::from(0u16),
            same_screen: true,
        };

        conn.send_event(false, window, EventMask::KEY_PRESS, &press)?;
        sleep(Duration::from_millis(KEY_DELAY_MS));
        conn.send_event(false, window, EventMask::KEY_RELEASE, &release)?;
        conn.flush()?;
    }
    Ok(())
}

/// Type a string with an additional `inter_char_ms` delay between each character.
pub fn send_type_text_with_delay(xid: u64, text: &str, inter_char_ms: u64) -> Result<()> {
    let (conn, _) = RustConnection::connect(None)?;
    let window = xid as u32;
    let root = conn.setup().roots[0].root;

    for ch in text.chars() {
        let keycode = char_to_keycode(&conn, ch).unwrap_or(0);
        if keycode == 0 { continue; }

        let press = KeyPressEvent {
            response_type: KEY_PRESS_EVENT,
            detail: keycode,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root, event: window, child: x11rb::NONE,
            root_x: 0, root_y: 0, event_x: 0, event_y: 0,
            state: KeyButMask::from(0u16),
            same_screen: true,
        };
        let release = KeyReleaseEvent {
            response_type: KEY_RELEASE_EVENT,
            detail: keycode,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root, event: window, child: x11rb::NONE,
            root_x: 0, root_y: 0, event_x: 0, event_y: 0,
            state: KeyButMask::from(0u16),
            same_screen: true,
        };

        conn.send_event(false, window, EventMask::KEY_PRESS, &press)?;
        sleep(Duration::from_millis(KEY_DELAY_MS));
        conn.send_event(false, window, EventMask::KEY_RELEASE, &release)?;
        conn.flush()?;
        if inter_char_ms > 0 {
            sleep(Duration::from_millis(inter_char_ms));
        }
    }
    Ok(())
}

/// Send a named key press to a window.
pub fn send_key(xid: u64, key: &str, modifiers: &[&str]) -> Result<()> {
    let (conn, _) = RustConnection::connect(None)?;
    let window = xid as u32;
    let root = conn.setup().roots[0].root;

    let keycode = key_name_to_keycode(&conn, key)?;
    let state = modifiers_to_state(modifiers);

    let press = KeyPressEvent {
        response_type: KEY_PRESS_EVENT,
        detail: keycode,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        root,
        event: window,
        child: x11rb::NONE,
        root_x: 0, root_y: 0,
        event_x: 0, event_y: 0,
        state,
        same_screen: true,
    };

    let release = KeyReleaseEvent {
        response_type: KEY_RELEASE_EVENT,
        detail: keycode,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        root,
        event: window,
        child: x11rb::NONE,
        root_x: 0, root_y: 0,
        event_x: 0, event_y: 0,
        state,
        same_screen: true,
    };

    conn.send_event(false, window, EventMask::KEY_PRESS, &press)?;
    sleep(Duration::from_millis(KEY_DELAY_MS));
    conn.send_event(false, window, EventMask::KEY_RELEASE, &release)?;
    conn.flush()?;
    Ok(())
}

fn char_to_keycode(conn: &RustConnection, ch: char) -> Option<u8> {
    // Use XStringToKeysym equivalent: look up by character keysym.
    // Keysym for ASCII is just the ASCII code.
    let keysym: u32 = ch as u32;
    conn.get_keyboard_mapping(8, 248).ok()?
        .reply().ok()
        .map(|km| {
            let keysyms_per = km.keysyms_per_keycode as usize;
            for (i, syms) in km.keysyms.chunks(keysyms_per).enumerate() {
                if syms.iter().any(|&s| s == keysym) {
                    return Some((8 + i) as u8);
                }
            }
            None
        })
        .flatten()
}

fn key_name_to_keycode(conn: &RustConnection, key: &str) -> Result<u8> {
    // Common X11 keysym names.
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
        "shift" => 0xFFE1, "ctrl" | "control" => 0xFFE3, "alt" => 0xFFE9, "super" | "meta" | "win" => 0xFFEB,
        "capslock" => 0xFFE5, "numlock" => 0xFF7F,
        s if s.len() == 1 => s.chars().next().unwrap() as u32,
        _ => anyhow::bail!("Unknown key: {key}"),
    };

    let km = conn.get_keyboard_mapping(8, 248)?.reply()?;
    let kpc = km.keysyms_per_keycode as usize;
    for (i, syms) in km.keysyms.chunks(kpc).enumerate() {
        if syms.iter().any(|&s| s == keysym) {
            return Ok((8 + i) as u8);
        }
    }
    anyhow::bail!("Keysym 0x{keysym:X} not in keyboard map for key '{key}'")
}

fn modifiers_to_state(modifiers: &[&str]) -> KeyButMask {
    let mut state = 0u16;
    for m in modifiers {
        match m.to_lowercase().as_str() {
            "shift" => state |= u16::from(KeyButMask::SHIFT),
            "ctrl" | "control" => state |= u16::from(KeyButMask::CONTROL),
            "alt" | "mod1" => state |= u16::from(KeyButMask::MOD1),
            "super" | "mod4" | "win" | "meta" => state |= u16::from(KeyButMask::MOD4),
            _ => {}
        }
    }
    KeyButMask::from(state)
}
