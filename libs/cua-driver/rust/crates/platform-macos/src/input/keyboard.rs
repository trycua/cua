//! Background keyboard synthesis via SLEventPostToPid (SkyLight SPI),
//! with fallback to the public CGEvent::post_to_pid.
//!
//! For keyboard events, `post_to_pid` attaches an `SLSEventAuthenticationMessage`
//! so Chromium-based apps accept synthetic keystrokes as trusted live input
//! (required on macOS 14+ for VS Code, Chrome, Electron apps).

use core_graphics::{
    event::{CGEvent, CGEventFlags},
    event_source::{CGEventSource, CGEventSourceStateID},
};
use foreign_types::ForeignType;

/// Press and release a single key, delivered to `pid` without stealing focus.
pub fn press_key(pid: i32, key: &str, modifiers: &[&str]) -> anyhow::Result<()> {
    // Handle "+" / "plus" → Shift+= (US keyboard layout).
    if key == "+" || key.to_lowercase() == "plus" {
        let flags = modifier_flags(&["shift"]);
        let eq_code = key_name_to_code("=")?;
        post_key(pid, eq_code, true,  modifier_flags(modifiers) | flags)?;
        std::thread::sleep(std::time::Duration::from_millis(8));
        post_key(pid, eq_code, false, modifier_flags(modifiers) | flags)?;
        return Ok(());
    }

    let key_code = key_name_to_code(key)?;
    let flags = modifier_flags(modifiers);

    post_key(pid, key_code, true, flags)?;
    std::thread::sleep(std::time::Duration::from_millis(8));
    post_key(pid, key_code, false, flags)?;
    Ok(())
}

/// Type a string character-by-character to `pid`.
pub fn type_text(pid: i32, text: &str) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;

    for ch in text.chars() {
        let ch_str = ch.to_string();
        let down = CGEvent::new_keyboard_event(source.clone(), 0, true)
            .map_err(|_| anyhow::anyhow!("CGEvent keyboard down failed"))?;
        down.set_string(&ch_str);
        // Always zero flags: Chrome inspects the flags field to infer modifier
        // state; without this, uppercase chars (e.g. 'E') are seen as Shift+e
        // and the modifier leaks into the next character (Swift fix: event.flags = []).
        down.set_flags(CGEventFlags::CGEventFlagNull);
        post_keyboard_event(pid, &down);
        std::thread::sleep(std::time::Duration::from_millis(8));

        let up = CGEvent::new_keyboard_event(source.clone(), 0, false)
            .map_err(|_| anyhow::anyhow!("CGEvent keyboard up failed"))?;
        up.set_string(&ch_str);
        up.set_flags(CGEventFlags::CGEventFlagNull);
        post_keyboard_event(pid, &up);
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    Ok(())
}

/// Type a string character-by-character with an extra `inter_char_delay_ms`
/// pause after each character (on top of the internal 8 ms down/up gap).
pub fn type_text_with_delay(pid: i32, text: &str, inter_char_delay_ms: u64) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;

    for ch in text.chars() {
        let ch_str = ch.to_string();
        let down = CGEvent::new_keyboard_event(source.clone(), 0, true)
            .map_err(|_| anyhow::anyhow!("CGEvent keyboard down failed"))?;
        down.set_string(&ch_str);
        down.set_flags(CGEventFlags::CGEventFlagNull);
        post_keyboard_event(pid, &down);
        std::thread::sleep(std::time::Duration::from_millis(8));

        let up = CGEvent::new_keyboard_event(source.clone(), 0, false)
            .map_err(|_| anyhow::anyhow!("CGEvent keyboard up failed"))?;
        up.set_string(&ch_str);
        up.set_flags(CGEventFlags::CGEventFlagNull);
        post_keyboard_event(pid, &up);

        // Additional inter-character delay on top of the 8 ms internal gap.
        if inter_char_delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(inter_char_delay_ms));
        } else {
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }
    Ok(())
}

/// Send a key combination (hotkey) to `pid`.
pub fn hotkey(pid: i32, key: &str, modifiers: &[&str]) -> anyhow::Result<()> {
    press_key(pid, key, modifiers)
}

/// Send a key combination to `pid` WITHOUT the auth-message envelope.
///
/// Required for NSMenu key equivalents: with the envelope, SLEventPostToPid
/// forks onto a direct-mach path that bypasses IOHIDPostEvent — NSMenu never
/// sees those events. Without the envelope the path goes through IOHIDPostEvent
/// so NSApplication.sendEvent: dispatches NSMenu key equivalents.
pub fn hotkey_no_auth(pid: i32, key: &str, modifiers: &[&str]) -> anyhow::Result<()> {
    let key_code = key_name_to_code(key)?;
    let flags = modifier_flags(modifiers);
    post_key_no_auth(pid, key_code, true, flags)?;
    std::thread::sleep(std::time::Duration::from_millis(8));
    post_key_no_auth(pid, key_code, false, flags)?;
    Ok(())
}

/// Press and release a single key to `pid` WITHOUT the auth-message envelope.
/// Works for single keys as well as combinations (same as hotkey_no_auth for single key).
pub fn press_key_no_auth(pid: i32, key: &str, modifiers: &[&str]) -> anyhow::Result<()> {
    let key_code = key_name_to_code(key)?;
    let flags = modifier_flags(modifiers);
    post_key_no_auth(pid, key_code, true, flags)?;
    std::thread::sleep(std::time::Duration::from_millis(8));
    post_key_no_auth(pid, key_code, false, flags)?;
    Ok(())
}

/// Post a keyboard event to `pid` via SLEventPostToPid (with auth message for
/// Chromium/Electron support) or fall back to CGEvent::post_to_pid.
fn post_keyboard_event(pid: i32, event: &CGEvent) {
    let event_ptr = event.as_ptr() as *mut std::ffi::c_void;
    // attachAuthMessage = true: required for Chromium keyboard on macOS 14+.
    if !crate::input::skylight::post_to_pid(pid as libc::pid_t, event_ptr, true) {
        event.post_to_pid(pid as libc::pid_t);
    }
}

fn post_key(pid: i32, key_code: u16, key_down: bool, flags: CGEventFlags) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let event = CGEvent::new_keyboard_event(source, key_code, key_down)
        .map_err(|_| anyhow::anyhow!("CGEvent::new_keyboard_event failed"))?;
    if flags != CGEventFlags::CGEventFlagNull {
        event.set_flags(flags);
    }
    post_keyboard_event(pid, &event);
    Ok(())
}

fn post_key_no_auth(pid: i32, key_code: u16, key_down: bool, flags: CGEventFlags) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let event = CGEvent::new_keyboard_event(source, key_code, key_down)
        .map_err(|_| anyhow::anyhow!("CGEvent::new_keyboard_event failed"))?;
    if flags != CGEventFlags::CGEventFlagNull {
        event.set_flags(flags);
    }
    let event_ptr = event.as_ptr() as *mut std::ffi::c_void;
    // attach_auth_message = false → IOHIDPostEvent path → NSMenu fires
    if !crate::input::skylight::post_to_pid(pid as libc::pid_t, event_ptr, false) {
        event.post_to_pid(pid as libc::pid_t);
    }
    Ok(())
}

fn modifier_flags(modifiers: &[&str]) -> CGEventFlags {
    let mut flags = CGEventFlags::CGEventFlagNull;
    for m in modifiers {
        match m.to_lowercase().as_str() {
            "cmd" | "command" => flags |= CGEventFlags::CGEventFlagCommand,
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "option" | "alt" => flags |= CGEventFlags::CGEventFlagAlternate,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            "fn" => flags |= CGEventFlags::CGEventFlagSecondaryFn,
            _ => {}
        }
    }
    flags
}

fn key_name_to_code(key: &str) -> anyhow::Result<u16> {
    let code = match key.to_lowercase().as_str() {
        "return" | "enter" => 36,
        "tab" => 48,
        "space" => 49,
        "delete" | "backspace" => 51,
        "escape" | "esc" => 53,
        "command" | "cmd" => 55,
        "shift" => 56,
        "capslock" => 57,
        "option" | "alt" => 58,
        "control" | "ctrl" => 59,
        "fn" => 63,
        "home" => 115,
        "pageup" => 116,
        "del" | "forward_delete" => 117,
        "end" => 119,
        "pagedown" => 121,
        "left" | "left_arrow" => 123,
        "right" | "right_arrow" => 124,
        "down" | "down_arrow" => 125,
        "up" | "up_arrow" => 126,
        "f1" => 122, "f2" => 120, "f3" => 99, "f4" => 118, "f5" => 96,
        "f6" => 97, "f7" => 98, "f8" => 100, "f9" => 101, "f10" => 109,
        "f11" => 103, "f12" => 111,
        "a" => 0, "s" => 1, "d" => 2, "f" => 3, "h" => 4, "g" => 5, "z" => 6, "x" => 7,
        "c" => 8, "v" => 9, "b" => 11, "q" => 12, "w" => 13, "e" => 14, "r" => 15, "y" => 16,
        "t" => 17, "1" => 18, "2" => 19, "3" => 20, "4" => 21, "6" => 22, "5" => 23, "=" => 24,
        "9" => 25, "7" => 26, "-" => 27, "8" => 28, "0" => 29, "]" => 30, "o" => 31, "u" => 32,
        "[" => 33, "i" => 34, "p" => 35, "l" => 37, "j" => 38, "'" => 39, "k" => 40, ";" => 41,
        "\\" => 42, "," => 43, "/" => 44, "n" => 45, "m" => 46, "." => 47, "`" => 50,
        _ => anyhow::bail!("Unknown key name: {key}"),
    };
    Ok(code)
}
