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

const SCREEN_SHARING_BUNDLE_ID: &str = "com.apple.ScreenSharing";
const SHIFT_KEY_CODE: u16 = 56;

fn is_screen_sharing_bundle_id(bundle_id: &str) -> bool {
    bundle_id == SCREEN_SHARING_BUNDLE_ID
}

/// Whether `pid` is Apple's Screen Sharing client.
///
/// Screen Sharing is an input forwarder rather than a text consumer: it relays
/// physical virtual-key transitions to the guest and ignores the Unicode string
/// attached to a synthetic keycode-0 event. Keep that special case explicit so
/// ordinary PID-routed text input retains its layout-independent Unicode path.
pub fn is_screen_sharing_pid(pid: i32) -> bool {
    crate::apps::bundle_id_for_pid(pid)
        .as_deref()
        .is_some_and(is_screen_sharing_bundle_id)
}

/// Press and release a single key, delivered to `pid` without stealing focus.
pub fn press_key(pid: i32, key: &str, modifiers: &[&str]) -> anyhow::Result<()> {
    // Handle "+" / "plus" → Shift+= (US keyboard layout).
    if key == "+" || key.to_lowercase() == "plus" {
        let flags = modifier_flags(&["shift"]);
        let eq_code = key_name_to_code("=")?;
        post_key(pid, eq_code, true, modifier_flags(modifiers) | flags)?;
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

/// Press and release one key on the global HID queue.
///
/// This is reserved for an explicitly approved, bounded foreground assist.
/// Callers must prove and temporarily activate the exact target window before
/// invoking it; unlike the PID-routed helpers above, the HID queue itself has
/// no process addressing.
pub fn press_key_global(key: &str, modifiers: &[&str]) -> anyhow::Result<()> {
    use core_graphics::event::CGEventTapLocation;

    let key_code = key_name_to_code(key)?;
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let mut active_flags = CGEventFlags::CGEventFlagNull;
    let mut pressed_modifiers: Vec<(u16, CGEventFlags)> = Vec::new();

    // A flag-only base-key event can leave the HID modifier state latched on
    // macOS. Model a physical chord instead: modifier downs in caller order,
    // base down/up, then modifier ups in reverse order. This also matches the
    // Windows SendInput and Linux XTest implementations.
    for modifier in modifiers {
        let Some((modifier_code, modifier_flag)) = modifier_key_code_and_flag(modifier) else {
            continue;
        };
        if pressed_modifiers
            .iter()
            .any(|(pressed_code, _)| *pressed_code == modifier_code)
        {
            continue;
        }
        active_flags |= modifier_flag;
        if let Err(error) = post_global_key(
            &source,
            modifier_code,
            true,
            active_flags,
            CGEventTapLocation::HID,
        ) {
            release_global_modifiers(
                &source,
                &pressed_modifiers,
                active_flags,
                CGEventTapLocation::HID,
            );
            return Err(error);
        }
        pressed_modifiers.push((modifier_code, modifier_flag));
        std::thread::sleep(std::time::Duration::from_millis(8));
    }

    let result = (|| {
        post_global_key(
            &source,
            key_code,
            true,
            active_flags,
            CGEventTapLocation::HID,
        )?;
        std::thread::sleep(std::time::Duration::from_millis(8));
        post_global_key(
            &source,
            key_code,
            false,
            active_flags,
            CGEventTapLocation::HID,
        )
    })();

    release_global_modifiers(
        &source,
        &pressed_modifiers,
        active_flags,
        CGEventTapLocation::HID,
    );
    result
}

fn post_global_key(
    source: &CGEventSource,
    key_code: u16,
    key_down: bool,
    flags: CGEventFlags,
    tap: core_graphics::event::CGEventTapLocation,
) -> anyhow::Result<()> {
    let event = CGEvent::new_keyboard_event(source.clone(), key_code, key_down)
        .map_err(|_| anyhow::anyhow!("CGEvent keyboard event creation failed"))?;
    event.set_flags(flags);
    event.post(tap);
    Ok(())
}

fn release_global_modifiers(
    source: &CGEventSource,
    pressed: &[(u16, CGEventFlags)],
    mut active_flags: CGEventFlags,
    tap: core_graphics::event::CGEventTapLocation,
) {
    for &(key_code, flag) in pressed.iter().rev() {
        active_flags.remove(flag);
        if let Ok(event) = CGEvent::new_keyboard_event(source.clone(), key_code, false) {
            event.set_flags(active_flags);
            event.post(tap);
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

fn modifier_key_code_and_flag(modifier: &str) -> Option<(u16, CGEventFlags)> {
    match modifier.to_lowercase().as_str() {
        "cmd" | "command" => Some((55, CGEventFlags::CGEventFlagCommand)),
        "shift" => Some((56, CGEventFlags::CGEventFlagShift)),
        "option" | "alt" => Some((58, CGEventFlags::CGEventFlagAlternate)),
        "ctrl" | "control" => Some((59, CGEventFlags::CGEventFlagControl)),
        "fn" => Some((63, CGEventFlags::CGEventFlagSecondaryFn)),
        _ => None,
    }
}

/// Type Unicode text into the frontmost application through the global HID
/// queue. This is the desktop-scope counterpart to PID-routed `type_text` and
/// mirrors computer-server's frontmost pynput typing behavior.
pub fn type_text_global(text: &str, inter_char_delay_ms: u64) -> anyhow::Result<()> {
    use core_graphics::event::CGEventTapLocation;

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    for ch in text.chars() {
        let value = ch.to_string();
        let down = CGEvent::new_keyboard_event(source.clone(), 0, true)
            .map_err(|_| anyhow::anyhow!("CGEvent keyboard down failed"))?;
        down.set_string(&value);
        down.set_flags(CGEventFlags::CGEventFlagNull);
        down.post(CGEventTapLocation::HID);
        std::thread::sleep(std::time::Duration::from_millis(8));
        let up = CGEvent::new_keyboard_event(source.clone(), 0, false)
            .map_err(|_| anyhow::anyhow!("CGEvent keyboard up failed"))?;
        up.set_string(&value);
        up.set_flags(CGEventFlags::CGEventFlagNull);
        up.post(CGEventTapLocation::HID);
        std::thread::sleep(std::time::Duration::from_millis(inter_char_delay_ms.max(8)));
    }
    Ok(())
}

/// Type ASCII text through the global HID queue as physical US-keyboard
/// transitions.
///
/// This is reserved for a caller that has guarded the exact target with
/// `with_foreground_hid_activation`. Remote-input clients such as Screen
/// Sharing forward virtual keycodes and modifier transitions, not the Unicode
/// payload carried by keycode 0, so the ordinary text synthesis path cannot be
/// used for them.
pub fn type_text_physical_global(text: &str, inter_char_delay_ms: u64) -> anyhow::Result<()> {
    use core_graphics::event::CGEventTapLocation;

    // Validate the complete payload before posting its first event. A string
    // containing an unsupported character must fail without partially typing.
    let event_groups = text
        .chars()
        .map(physical_text_events)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    for events in event_groups {
        for event in events {
            let cg_event =
                CGEvent::new_keyboard_event(source.clone(), event.key_code, event.key_down)
                    .map_err(|_| anyhow::anyhow!("CGEvent keyboard event creation failed"))?;
            if let Some(value) = event.text {
                cg_event.set_string(&value.to_string());
            }
            cg_event.set_flags(if event.shift {
                CGEventFlags::CGEventFlagShift
            } else {
                CGEventFlags::CGEventFlagNull
            });
            cg_event.post(CGEventTapLocation::HID);
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        if inter_char_delay_ms > 8 {
            std::thread::sleep(std::time::Duration::from_millis(inter_char_delay_ms - 8));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PhysicalTextEvent {
    key_code: u16,
    key_down: bool,
    shift: bool,
    text: Option<char>,
}

fn physical_text_events(ch: char) -> anyhow::Result<Vec<PhysicalTextEvent>> {
    let (key_code, shift) = physical_key_for_char(ch).ok_or_else(|| {
        anyhow::anyhow!(
            "Screen Sharing physical text delivery does not support character U+{:04X}",
            ch as u32
        )
    })?;
    let mut events = Vec::with_capacity(if shift { 4 } else { 2 });
    if shift {
        events.push(PhysicalTextEvent {
            key_code: SHIFT_KEY_CODE,
            key_down: true,
            shift: true,
            text: None,
        });
    }
    events.push(PhysicalTextEvent {
        key_code,
        key_down: true,
        shift,
        text: Some(ch),
    });
    events.push(PhysicalTextEvent {
        key_code,
        key_down: false,
        shift,
        text: Some(ch),
    });
    if shift {
        events.push(PhysicalTextEvent {
            key_code: SHIFT_KEY_CODE,
            key_down: false,
            shift: false,
            text: None,
        });
    }
    Ok(events)
}

/// Map printable ASCII to the physical key that produces it on the standard US
/// layout. The Unicode payload remains attached to the base key for local event
/// consumers, while forwarders can use the keycode and Shift transitions.
fn physical_key_for_char(ch: char) -> Option<(u16, bool)> {
    let lower = ch.to_ascii_lowercase();
    if ch.is_ascii_alphabetic() {
        return key_name_to_code(&lower.to_string())
            .ok()
            .map(|code| (code, ch.is_ascii_uppercase()));
    }
    if ch.is_ascii_digit() {
        return key_name_to_code(&ch.to_string())
            .ok()
            .map(|code| (code, false));
    }

    let (base, shift) = match ch {
        ' ' => ("space", false),
        '\t' => ("tab", false),
        '\n' | '\r' => ("return", false),
        '-' => ("-", false),
        '_' => ("-", true),
        '=' => ("=", false),
        '+' => ("=", true),
        '[' => ("[", false),
        '{' => ("[", true),
        ']' => ("]", false),
        '}' => ("]", true),
        '\\' => ("\\", false),
        '|' => ("\\", true),
        ';' => (";", false),
        ':' => (";", true),
        '\'' => ("'", false),
        '"' => ("'", true),
        ',' => (",", false),
        '<' => (",", true),
        '.' => (".", false),
        '>' => (".", true),
        '/' => ("/", false),
        '?' => ("/", true),
        '`' => ("`", false),
        '~' => ("`", true),
        '!' => ("1", true),
        '@' => ("2", true),
        '#' => ("3", true),
        '$' => ("4", true),
        '%' => ("5", true),
        '^' => ("6", true),
        '&' => ("7", true),
        '*' => ("8", true),
        '(' => ("9", true),
        ')' => ("0", true),
        _ => return None,
    };
    key_name_to_code(base).ok().map(|code| (code, shift))
}

/// Post a keyboard event to `pid` via SLEventPostToPid (with auth message for
/// Chromium/Electron support) or fall back to CGEvent::post_to_pid.
pub(super) fn post_keyboard_event(pid: i32, event: &CGEvent) {
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
    // HIDSystemState can inherit physically held modifiers. Always overwrite
    // the event flags, including the empty case, so an unrelated Shift/Caps
    // state cannot leak into a targeted key press.
    event.set_flags(flags);
    post_keyboard_event(pid, &event);
    Ok(())
}

fn post_key_no_auth(
    pid: i32,
    key_code: u16,
    key_down: bool,
    flags: CGEventFlags,
) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let event = CGEvent::new_keyboard_event(source, key_code, key_down)
        .map_err(|_| anyhow::anyhow!("CGEvent::new_keyboard_event failed"))?;
    event.set_flags(flags);
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

pub(super) fn key_name_to_code(key: &str) -> anyhow::Result<u16> {
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
        "f1" => 122,
        "f2" => 120,
        "f3" => 99,
        "f4" => 118,
        "f5" => 96,
        "f6" => 97,
        "f7" => 98,
        "f8" => 100,
        "f9" => 101,
        "f10" => 109,
        "f11" => 103,
        "f12" => 111,
        "a" => 0,
        "s" => 1,
        "d" => 2,
        "f" => 3,
        "h" => 4,
        "g" => 5,
        "z" => 6,
        "x" => 7,
        "c" => 8,
        "v" => 9,
        "b" => 11,
        "q" => 12,
        "w" => 13,
        "e" => 14,
        "r" => 15,
        "y" => 16,
        "t" => 17,
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "=" => 24,
        "9" => 25,
        "7" => 26,
        "-" => 27,
        "8" => 28,
        "0" => 29,
        "]" => 30,
        "o" => 31,
        "u" => 32,
        "[" => 33,
        "i" => 34,
        "p" => 35,
        "l" => 37,
        "j" => 38,
        "'" => 39,
        "k" => 40,
        ";" => 41,
        "\\" => 42,
        "," => 43,
        "/" => 44,
        "n" => 45,
        "m" => 46,
        "." => 47,
        "`" => 50,
        _ => anyhow::bail!("Unknown key name: {key}"),
    };
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_text_uses_mixed_keycodes_and_balanced_shift_transitions() {
        assert_eq!(
            physical_text_events('a').unwrap(),
            vec![
                PhysicalTextEvent {
                    key_code: 0,
                    key_down: true,
                    shift: false,
                    text: Some('a'),
                },
                PhysicalTextEvent {
                    key_code: 0,
                    key_down: false,
                    shift: false,
                    text: Some('a'),
                },
            ]
        );
        assert_eq!(
            physical_text_events('Z').unwrap(),
            vec![
                PhysicalTextEvent {
                    key_code: 56,
                    key_down: true,
                    shift: true,
                    text: None,
                },
                PhysicalTextEvent {
                    key_code: 6,
                    key_down: true,
                    shift: true,
                    text: Some('Z'),
                },
                PhysicalTextEvent {
                    key_code: 6,
                    key_down: false,
                    shift: true,
                    text: Some('Z'),
                },
                PhysicalTextEvent {
                    key_code: 56,
                    key_down: false,
                    shift: false,
                    text: None,
                },
            ]
        );
        assert_eq!(physical_key_for_char('1'), Some((18, false)));
        assert_eq!(physical_key_for_char('!'), Some((18, true)));
        assert_eq!(physical_key_for_char('/'), Some((44, false)));
        assert_eq!(physical_key_for_char('?'), Some((44, true)));
    }

    #[test]
    fn physical_text_rejects_characters_without_a_lossless_keycode() {
        let error = physical_text_events('🙂').unwrap_err().to_string();
        assert!(error.contains("U+1F642"), "{error}");
    }

    #[test]
    fn physical_text_covers_printable_ascii_and_common_text_whitespace() {
        for byte in 0x20_u8..=0x7e {
            let ch = char::from(byte);
            assert!(
                physical_key_for_char(ch).is_some(),
                "missing physical key for ASCII {ch:?}"
            );
        }
        for ch in ['\t', '\n', '\r'] {
            assert!(
                physical_key_for_char(ch).is_some(),
                "missing physical key for whitespace {ch:?}"
            );
        }
    }

    #[test]
    fn screen_sharing_detection_is_exact_and_case_sensitive() {
        assert!(is_screen_sharing_bundle_id("com.apple.ScreenSharing"));
        assert!(!is_screen_sharing_bundle_id("com.apple.screensharing"));
        assert!(!is_screen_sharing_bundle_id("com.microsoft.rdc.macos"));
    }
}
