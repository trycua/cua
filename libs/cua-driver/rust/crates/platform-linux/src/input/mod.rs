//! Background input injection for Linux via X11 XSendEvent.
//!
//! XSendEvent sends synthetic events directly to a window without changing
//! input focus — the Linux equivalent of PostMessage on Windows, and the
//! mechanism behind the cross-platform "no focus steal" contract.
//!
//! Note: a few apps check the `send_event` flag and ignore synthetic events.
//! Terminal emulators are the notable case (xterm's `allowSendEvents` is off by
//! default); those are handled out of band by writing to the pty master — see
//! `crate::tty`. We deliberately do NOT fall back to the XTest extension for
//! them, because XTest delivers to the *focused* window and would break the
//! no-focus-steal contract.

use anyhow::Result;
use std::thread::sleep;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

const CLICK_DELAY_MS: u64 = 35;
const KEY_DELAY_MS: u64 = 10;

/// Send a synthetic FocusIn event to a window without changing the actual X11 input focus.
/// This can trigger toolkit-level focus handlers (e.g., Qt5's AT-SPI bridge) without
/// moving the window manager's active window. Use with send_focus_out to restore state.
pub fn send_focus_in(xid: u64) -> Result<()> {
    let (conn, _) = RustConnection::connect(None)?;
    let window = xid as u32;

    let focus_in = FocusInEvent {
        response_type: FOCUS_IN_EVENT,
        detail: NotifyDetail::NONLINEAR,
        sequence: 0,
        event: window,
        mode: NotifyMode::NORMAL,
    };

    conn.send_event(false, window, EventMask::FOCUS_CHANGE, &focus_in)?;
    conn.flush()?;
    Ok(())
}

/// Send a synthetic FocusOut event to restore focus state after send_focus_in.
pub fn send_focus_out(xid: u64) -> Result<()> {
    let (conn, _) = RustConnection::connect(None)?;
    let window = xid as u32;

    let focus_out = FocusOutEvent {
        response_type: FOCUS_OUT_EVENT,
        detail: NotifyDetail::NONLINEAR,
        sequence: 0,
        event: window,
        mode: NotifyMode::NORMAL,
    };

    conn.send_event(false, window, EventMask::FOCUS_CHANGE, &focus_out)?;
    conn.flush()?;
    Ok(())
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

/// Type a string by sending KeyPress/KeyRelease events for each character.
pub fn send_type_text(xid: u64, text: &str) -> Result<()> {
    send_type_text_with_delay(xid, text, 0)
}

/// Type a string with an additional `inter_char_ms` delay between each character.
pub fn send_type_text_with_delay(xid: u64, text: &str, inter_char_ms: u64) -> Result<()> {
    let (conn, _) = RustConnection::connect(None)?;
    let window = xid as u32;
    let root = conn.setup().roots[0].root;
    let mapping = conn.get_keyboard_mapping(8, 248)?.reply()?;

    for ch in text.chars() {
        // Resolve the keycode and whether Shift must be held — without it,
        // uppercase and shifted symbols would otherwise type their unshifted
        // form (e.g. "A" arriving as "a").
        let Some((keycode, needs_shift)) = char_to_keycode_shift(&mapping, ch as u32) else {
            continue;
        };
        let state = if needs_shift { KeyButMask::SHIFT } else { KeyButMask::from(0u16) };

        let press = KeyPressEvent {
            response_type: KEY_PRESS_EVENT,
            detail: keycode,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root, event: window, child: x11rb::NONE,
            root_x: 0, root_y: 0, event_x: 0, event_y: 0,
            state,
            same_screen: true,
        };
        let release = KeyReleaseEvent {
            response_type: KEY_RELEASE_EVENT,
            detail: keycode,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root, event: window, child: x11rb::NONE,
            root_x: 0, root_y: 0, event_x: 0, event_y: 0,
            state,
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

/// Find the keycode that emits `keysym`, plus whether Shift must be held (the
/// keysym sits in the shifted column of the keyboard map). Prefers the
/// unshifted column when a keysym appears in both. Keysym for ASCII / Latin-1
/// is just the codepoint.
fn char_to_keycode_shift(mapping: &GetKeyboardMappingReply, keysym: u32) -> Option<(u8, bool)> {
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

/// Inject text into a Tk window via Tk's `send` command — the Tk-specific
/// override for focus-free writes (Tk has no AT-SPI bridge). Requires the target
/// app to have registered itself with a known name via `tk appname <name>`.
/// Returns Ok(true) if text was sent, Ok(false) if the target isn't reachable
/// (not a Tk app or `wish` unavailable), Err on a send failure.
pub fn inject_tk_send(text: &str) -> Result<bool> {
    use std::io::Write;

    // Escape the text for safe Tcl interpolation (braces for literal strings).
    // Tcl's `send` command: `send <target-app-name> <tcl-command>`.
    // We target "cua-tk-target" (the name the test app registers with) and
    // insert at the entry widget's current cursor position.
    let tcl_text = text.replace("\\", "\\\\").replace("{", "\\{").replace("}", "\\}");

    // Tk's `send` is synchronous: it blocks the sender until the *target's* Tcl
    // event loop services the request and replies. If the target is wedged, or
    // the X server refuses `send` (SECURITY ext / xauth mismatch), it can block
    // forever. Guard against that two ways:
    //   1. A Tcl-level `after` timer that force-exits wish if the send hasn't
    //      completed in time. We issue the write with `send -async` so the local
    //      event loop stays live to fire the timer, then `vwait` on a flag.
    //   2. A Rust-level wall-clock kill below, so even a totally wedged wish
    //      (e.g. blocked before reaching the event loop) can't hang the driver.
    let tcl_script = format!(
        r#"set ::done 0
set ::rc 0
after 5000 {{ set ::rc 2; set ::done 1 }}
if {{[catch {{send -async cua-tk-target {{.entry insert insert {{{}}}}}}} err]}} {{
    puts stderr "tk send failed: $err"
    exit 1
}}
after 500 {{ set ::done 1 }}
vwait ::done
if {{$::rc == 2}} {{
    puts stderr "tk send timed out"
    exit 1
}}
exit 0"#,
        tcl_text
    );

    // Try to spawn wish (Tk's shell). If it's not available, this isn't a
    // Tk-based environment and we should fall back to XSendEvent.
    let mut child = match std::process::Command::new("wish")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn() {
            Ok(c) => c,
            Err(_) => return Ok(false),
        };

    if let Some(mut stdin) = child.stdin.take() {
        // Ignore write errors: if wish already exited we observe it via wait().
        let _ = stdin.write_all(tcl_script.as_bytes());
        // stdin drops here → EOF, so wish runs the script to completion.
    }

    // Wall-clock backstop: poll for exit and hard-kill if wish overruns the
    // deadline. Guarantees the driver task can never hang on a blocked Tk send,
    // regardless of whether the Tcl-level timer fired.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        match child.try_wait()? {
            Some(status) => {
                let mut stderr = String::new();
                if let Some(mut err) = child.stderr.take() {
                    use std::io::Read;
                    let _ = err.read_to_string(&mut stderr);
                }
                if status.success() {
                    return Ok(true);
                }
                // Target not registered or send timed out → not a usable Tk
                // target; let the caller fall back to XSendEvent.
                if stderr.contains("application named")
                    || stderr.contains("no registered")
                    || stderr.contains("timed out")
                {
                    return Ok(false);
                }
                anyhow::bail!("wish send failed: {}", stderr);
            }
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}
