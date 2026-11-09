// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

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
use std::sync::{Arc, Mutex};

use super::input_source;

const SCREEN_SHARING_BUNDLE_ID: &str = "com.apple.ScreenSharing";
const SHIFT_KEY_CODE: u16 = 56;

/// Default key down→up gap and inter-key pacing.
const DEFAULT_KEY_GAP: std::time::Duration = std::time::Duration::from_millis(8);

/// Key down→up gap and inter-key pacing: `DEFAULT_KEY_GAP`, or the
/// host's `CUA_DRIVER_KEY_GAP_MS` clamped by `cua_driver_core::key_pacing`.
/// Read once: pacing is a launch-time knob, not a per-call one.
fn key_gap() -> std::time::Duration {
    static GAP: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
    *GAP.get_or_init(|| cua_driver_core::key_pacing::key_gap_from_env(DEFAULT_KEY_GAP))
}

fn key_gap_sleep() {
    std::thread::sleep(key_gap());
}

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
        let eq_code = target_key_code(pid, "=", modifiers)?;
        post_key(pid, eq_code, true, modifier_flags(modifiers) | flags)?;
        key_gap_sleep();
        post_key(pid, eq_code, false, modifier_flags(modifiers) | flags)?;
        return Ok(());
    }

    let key_code = target_key_code(pid, key, modifiers)?;
    let flags = modifier_flags(modifiers);

    post_key(pid, key_code, true, flags)?;
    key_gap_sleep();
    post_key(pid, key_code, false, flags)?;
    Ok(())
}

/// The virtual key a control character in typed text stands for.
///
/// Typed text is otherwise sent as a Unicode payload on key code 0 (the A
/// key). For `\n`, `\r` and `\t` that is not a key press at all: Chrome's
/// omnibox ignored the `\n` of `file:///…log\n` and never navigated (bench
/// CDB-G04, CUA-1223), and LibreOffice did not commit a Name Box entry.
/// These are sent as the real Return and Tab keys instead.
fn control_key_for_char(ch: char) -> Option<u16> {
    match ch {
        '\n' | '\r' => key_name_to_code("return").ok(),
        '\t' => key_name_to_code("tab").ok(),
        _ => None,
    }
}

/// The down/up pair that types `ch`: its control key when it has one,
/// otherwise a Unicode payload on key code 0. Flags are always zero: Chrome
/// reads the flags field to infer modifier state, and without this an
/// uppercase char (e.g. 'E') is seen as Shift+e and the modifier leaks into
/// the next character (Swift fix: event.flags = []).
fn char_key_events(source: &CGEventSource, ch: char) -> anyhow::Result<(CGEvent, CGEvent)> {
    match control_key_for_char(ch) {
        Some(code) => key_events(source, code, None),
        None => key_events(source, 0, Some(ch.encode_utf8(&mut [0; 4]))),
    }
}

fn key_events(
    source: &CGEventSource,
    key_code: u16,
    payload: Option<&str>,
) -> anyhow::Result<(CGEvent, CGEvent)> {
    let make = |down: bool| -> anyhow::Result<CGEvent> {
        let event = CGEvent::new_keyboard_event(source.clone(), key_code, down)
            .map_err(|_| anyhow::anyhow!("CGEvent keyboard event failed"))?;
        if let Some(payload) = payload {
            event.set_string(payload);
        }
        event.set_flags(CGEventFlags::CGEventFlagNull);
        Ok(event)
    };
    Ok((make(true)?, make(false)?))
}

/// Most UTF-16 units one keyboard event may carry
/// (`CGEventKeyboardSetUnicodeString` documents a 20-unit ceiling).
pub const MAX_UNICODE_UNITS_PER_EVENT: usize = 16;

/// One keyboard event pair of batched typing.
#[derive(Debug, PartialEq, Eq)]
pub enum TypingChunk<'a> {
    /// Plain text carried as one Unicode payload on a single key pair.
    Text(&'a str),
    /// A control character sent as its real Return/Tab key.
    Control(char),
}

/// Split `text` into the fewest key-event pairs: runs of ordinary characters
/// of at most [`MAX_UNICODE_UNITS_PER_EVENT`] UTF-16 units (never splitting a
/// character), with `\n`, `\r` and `\t` kept as their own real key presses.
pub fn plan_chunks(text: &str) -> Vec<TypingChunk<'_>> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut units = 0;
    for (i, ch) in text.char_indices() {
        if control_key_for_char(ch).is_some() {
            if start < i {
                chunks.push(TypingChunk::Text(&text[start..i]));
            }
            chunks.push(TypingChunk::Control(ch));
            start = i + ch.len_utf8();
            units = 0;
            continue;
        }
        let n = ch.len_utf16();
        if units + n > MAX_UNICODE_UNITS_PER_EVENT {
            chunks.push(TypingChunk::Text(&text[start..i]));
            start = i;
            units = 0;
        }
        units += n;
    }
    if start < text.len() {
        chunks.push(TypingChunk::Text(&text[start..]));
    }
    chunks
}

/// Type `text` to `pid` in batches: one key-down/key-up pair per
/// [`TypingChunk`] instead of per character, with no inter-character sleep.
/// A 100-character string costs about 7 pairs rather than 100.
pub fn type_text_batched(pid: i32, text: &str) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    for chunk in plan_chunks(text) {
        let (down, up) = match chunk {
            TypingChunk::Text(run) => key_events(&source, 0, Some(run))?,
            TypingChunk::Control(ch) => char_key_events(&source, ch)?,
        };
        post_keyboard_event(pid, &down);
        key_gap_sleep();
        post_keyboard_event(pid, &up);
        key_gap_sleep();
    }
    Ok(())
}

/// Type a string character-by-character to `pid`.
pub fn type_text(pid: i32, text: &str) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;

    for ch in text.chars() {
        let (down, up) = char_key_events(&source, ch)?;
        post_keyboard_event(pid, &down);
        key_gap_sleep();
        post_keyboard_event(pid, &up);
        key_gap_sleep();
    }
    Ok(())
}

/// Type a string character-by-character with an extra `inter_char_delay_ms`
/// pause after each character (on top of the internal down/up key gap).
///
/// `inter_char_delay_ms == 0` selects [`type_text_batched`]: a caller that asks
/// for no pacing gets the fast path; any positive delay keeps one event pair
/// per character so debounce-sensitive fields see each key.
pub fn type_text_with_delay(pid: i32, text: &str, inter_char_delay_ms: u64) -> anyhow::Result<()> {
    if inter_char_delay_ms == 0 {
        return type_text_batched(pid, text);
    }
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;

    for ch in text.chars() {
        let (down, up) = char_key_events(&source, ch)?;
        post_keyboard_event(pid, &down);
        key_gap_sleep();
        post_keyboard_event(pid, &up);

        // Additional inter-character delay on top of the internal key gap.
        if inter_char_delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(inter_char_delay_ms));
        } else {
            key_gap_sleep();
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
    let key_code = target_key_code(pid, key, modifiers)?;
    let flags = modifier_flags(modifiers);
    post_key_no_auth(pid, key_code, true, flags)?;
    key_gap_sleep();
    post_key_no_auth(pid, key_code, false, flags)?;
    Ok(())
}

/// Press and release a single key to `pid` WITHOUT the auth-message envelope.
/// Works for single keys as well as combinations (same as hotkey_no_auth for single key).
pub fn press_key_no_auth(pid: i32, key: &str, modifiers: &[&str]) -> anyhow::Result<()> {
    let key_code = target_key_code(pid, key, modifiers)?;
    let flags = modifier_flags(modifiers);
    post_key_no_auth(pid, key_code, true, flags)?;
    key_gap_sleep();
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

    // The HID queue reaches the frontmost app, which may be Screen Sharing.
    let key_code = target_key_code(crate::apps::frontmost_pid(), key, modifiers)?;
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
        key_gap_sleep();
    }

    let result = (|| {
        post_global_key(
            &source,
            key_code,
            true,
            active_flags,
            CGEventTapLocation::HID,
        )?;
        key_gap_sleep();
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

/// Hold physical modifier keys on the global HID queue around one pointer
/// gesture, then release them in reverse order even when the gesture fails.
///
/// This is only safe inside an exact-window foreground activation guard. The
/// caller receives the live modifier flags so every mouse event in the gesture
/// carries the same state as the physical key transitions.
pub(super) fn with_global_modifier_keys<T>(
    modifiers: &[&str],
    gesture: impl FnOnce(CGEventFlags) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    use core_graphics::event::CGEventTapLocation;

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let mut active_flags = CGEventFlags::CGEventFlagNull;
    let mut pressed = Vec::new();

    for modifier in modifiers {
        let Some((key_code, flag)) = modifier_key_code_and_flag(modifier) else {
            continue;
        };
        if pressed
            .iter()
            .any(|(pressed_code, _)| *pressed_code == key_code)
        {
            continue;
        }
        active_flags |= flag;
        if let Err(error) = post_global_key(
            &source,
            key_code,
            true,
            active_flags,
            CGEventTapLocation::HID,
        ) {
            release_global_modifiers(&source, &pressed, active_flags, CGEventTapLocation::HID);
            return Err(error);
        }
        pressed.push((key_code, flag));
        key_gap_sleep();
    }

    let result = gesture(active_flags);
    release_global_modifiers(&source, &pressed, active_flags, CGEventTapLocation::HID);
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
        key_gap_sleep();
    }
}

fn modifier_key_code_and_flag(modifier: &str) -> Option<(u16, CGEventFlags)> {
    match modifier.to_lowercase().as_str() {
        "cmd" | "command" | "super" | "meta" | "win" => {
            Some((55, CGEventFlags::CGEventFlagCommand))
        }
        "shift" => Some((56, CGEventFlags::CGEventFlagShift)),
        "option" | "alt" => Some((58, CGEventFlags::CGEventFlagAlternate)),
        "ctrl" | "control" => Some((59, CGEventFlags::CGEventFlagControl)),
        "fn" => Some((63, CGEventFlags::CGEventFlagSecondaryFn)),
        _ => None,
    }
}

/// Hold PID-routed modifier keys around one pointer gesture and release them in
/// reverse order even when the gesture fails.
///
/// Mouse-event flag bits alone are not a sufficient physical-modifier model for
/// every AppKit host (Finder's collection views are a notable example). Pairing
/// the flagged mouse events with real modifier down/up transitions gives the
/// target the same ordered state it receives for a keyboard chord without
/// putting those transitions on the global HID queue.
pub(super) fn with_pid_modifier_keys<T>(
    pid: i32,
    modifiers: &[&str],
    gesture: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let mut active_flags = CGEventFlags::CGEventFlagNull;
    let mut pressed = Vec::new();
    for modifier in modifiers {
        let Some((key_code, flag)) = modifier_key_code_and_flag(modifier) else {
            continue;
        };
        if pressed
            .iter()
            .any(|(pressed_code, _)| *pressed_code == key_code)
        {
            continue;
        }
        active_flags |= flag;
        if let Err(error) = post_key(pid, key_code, true, active_flags) {
            release_pid_modifiers(pid, &pressed, active_flags);
            return Err(error);
        }
        pressed.push((key_code, flag));
        key_gap_sleep();
    }

    let result = gesture();
    release_pid_modifiers(pid, &pressed, active_flags);
    result
}

fn release_pid_modifiers(
    pid: i32,
    pressed: &[(u16, CGEventFlags)],
    mut active_flags: CGEventFlags,
) {
    for &(key_code, flag) in pressed.iter().rev() {
        active_flags.remove(flag);
        let _ = post_key(pid, key_code, false, active_flags);
        key_gap_sleep();
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
        let (down, up) = char_key_events(&source, ch)?;
        down.post(CGEventTapLocation::HID);
        key_gap_sleep();
        up.post(CGEventTapLocation::HID);
        std::thread::sleep(std::time::Duration::from_millis(inter_char_delay_ms).max(key_gap()));
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
    for events in event_groups {
        let native_events = events
            .iter()
            .map(|event| create_bare_keyboard_event(event.key_code, event.key_down))
            .collect::<anyhow::Result<Vec<_>>>()?;
        for cg_event in native_events {
            cg_event.post(CGEventTapLocation::HID);
            key_gap_sleep();
        }
        let extra = std::time::Duration::from_millis(inter_char_delay_ms).saturating_sub(key_gap());
        if !extra.is_zero() {
            std::thread::sleep(extra);
        }
    }
    Ok(())
}

/// Send a physical key chord as the virtual-key transition sequence Apple
/// documents for `CGEventCreateKeyboardEvent`: NULL source, modifier downs,
/// base down/up, then modifier ups in reverse order.
///
/// Each transition also carries the chord's accumulated `CGEventFlags`, as the
/// PID-routed and global rungs do. A keyboard event created from the default
/// source starts with no flags, and posting a modifier keycode does not
/// retro-fit them onto events that were already created, so a chord whose base
/// key carries no flags arrives as the bare key: `cmd+a` inserts a literal `a`.
/// Remote-input clients such as Screen Sharing re-derive modifiers from the
/// keycode transitions and ignore the flags, so both consumers are served.
pub fn press_key_bare_global(pid: i32, key: &str, modifiers: &[&str]) -> anyhow::Result<()> {
    use core_graphics::event::CGEventTapLocation;

    let key_code = target_key_code(pid, key, modifiers)?;
    let mut modifier_keys = Vec::new();
    for modifier in modifiers {
        let Some((modifier_code, flag)) = modifier_key_code_and_flag(modifier) else {
            continue;
        };
        if !modifier_keys.iter().any(|&(code, _)| code == modifier_code) {
            modifier_keys.push((modifier_code, flag));
        }
    }

    let events = bare_chord_transitions(key_code, &modifier_keys)
        .into_iter()
        .map(|(code, down, flags)| {
            let event = create_bare_keyboard_event(code, down)?;
            event.set_flags(flags);
            Ok(event)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    for event in events {
        event.post(CGEventTapLocation::HID);
        key_gap_sleep();
    }
    Ok(())
}

/// The chord's transitions with the flags each one carries. A modifier's own
/// down is the first event that holds its flag and its up is the first that has
/// dropped it again, so the base key always sees the full chord.
fn bare_chord_transitions(
    key_code: u16,
    modifier_keys: &[(u16, CGEventFlags)],
) -> Vec<(u16, bool, CGEventFlags)> {
    let mut transitions = Vec::with_capacity(modifier_keys.len() * 2 + 2);
    let mut flags = CGEventFlags::CGEventFlagNull;
    let mut releases = Vec::with_capacity(modifier_keys.len());
    for &(code, flag) in modifier_keys {
        let previous = flags;
        flags |= flag;
        transitions.push((code, true, flags));
        releases.push((code, false, previous));
    }
    transitions.push((key_code, true, flags));
    transitions.push((key_code, false, flags));
    transitions.extend(releases.into_iter().rev());
    transitions
}

fn create_bare_keyboard_event(key_code: u16, key_down: bool) -> anyhow::Result<CGEvent> {
    unsafe {
        let event_ref = CGEventCreateKeyboardEvent(std::ptr::null_mut(), key_code, key_down);
        if event_ref.is_null() {
            anyhow::bail!("CGEventCreateKeyboardEvent with default source failed");
        }
        Ok(CGEvent::from_ptr(event_ref))
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreateKeyboardEvent(
        source: core_graphics::sys::CGEventSourceRef,
        key_code: u16,
        key_down: bool,
    ) -> core_graphics::sys::CGEventRef;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PhysicalTextEvent {
    key_code: u16,
    key_down: bool,
    event_type: PhysicalEventType,
    shift: bool,
    text: Option<char>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PhysicalEventType {
    KeyDown,
    KeyUp,
    FlagsChanged,
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
            event_type: PhysicalEventType::FlagsChanged,
            shift: true,
            text: None,
        });
    }
    events.push(PhysicalTextEvent {
        key_code,
        key_down: true,
        event_type: PhysicalEventType::KeyDown,
        shift,
        text: Some(ch),
    });
    events.push(PhysicalTextEvent {
        key_code,
        key_down: false,
        event_type: PhysicalEventType::KeyUp,
        shift,
        text: Some(ch),
    });
    if shift {
        events.push(PhysicalTextEvent {
            key_code: SHIFT_KEY_CODE,
            key_down: false,
            event_type: PhysicalEventType::FlagsChanged,
            shift: false,
            text: None,
        });
    }
    Ok(events)
}

/// Map printable ASCII to the physical key that produces it on the standard US
/// layout. The Screen Sharing path intentionally sends only the returned
/// keycode and the required bare Shift transitions.
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
            "cmd" | "command" | "super" | "meta" | "win" => {
                flags |= CGEventFlags::CGEventFlagCommand
            }
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "option" | "alt" => flags |= CGEventFlags::CGEventFlagAlternate,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            "fn" => flags |= CGEventFlags::CGEventFlagSecondaryFn,
            _ => {}
        }
    }
    flags
}

/// A multi-character key name as `key_name_to_code` matches it: lower case
/// with spaces, `_` and `-` dropped, and common spellings folded onto one
/// name, so "Page_Down", "page-down", "PgDn" and "ArrowLeft" all work. Single
/// characters ("-", "_" never reaches here as a name) pass through unchanged.
fn canonical_key_name(key: &str) -> String {
    let lower = key.trim().to_lowercase();
    if lower.chars().count() < 2 {
        return lower;
    }
    let compact: String = lower
        .chars()
        .filter(|c| !matches!(c, ' ' | '_' | '-'))
        .collect();
    match compact.as_str() {
        "pgdn" | "pgdown" => "pagedown",
        "pgup" => "pageup",
        "arrowleft" | "leftarrow" => "left",
        "arrowright" | "rightarrow" => "right",
        "arrowup" | "uparrow" => "up",
        "arrowdown" | "downarrow" => "down",
        "forwarddelete" | "fwddelete" | "deleteforward" => "forward_delete",
        "bksp" => "backspace",
        "escape" => "escape",
        "kpenter" | "numpadenter" => "enter",
        _ => return compact,
    }
    .to_owned()
}

pub(super) fn key_name_to_code(key: &str) -> anyhow::Result<u16> {
    let code = match canonical_key_name(key).as_str() {
        "return" | "enter" => 36,
        "tab" => 48,
        "space" => 49,
        "delete" | "backspace" => 51,
        "escape" | "esc" => 53,
        "command" | "cmd" | "super" | "meta" | "win" => 55,
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
        _ => anyhow::bail!("{}", unknown_key_message(key)),
    };
    Ok(code)
}

/// The characters a keyboard layout types, by virtual key code: with no
/// modifier, with Command held and with Control held. The Command layer is
/// QWERTY on Russian and on Dvorak - QWERTY ⌘ and the same as the plain one
/// on AZERTY and QWERTZ; the Control layer types control characters (`^C` for
/// the key that types `c`) on most layouts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KeyLayout {
    plain: Vec<(u16, char)>,
    command: Vec<(u16, char)>,
    control: Vec<(u16, char)>,
}

impl KeyLayout {
    /// Return and Tab type `^M` and `^I` with Control held: they are left out
    /// of the Control layer so `ctrl+m` presses the M key.
    fn new(plain: Vec<(u16, char)>, command: Vec<(u16, char)>, control: Vec<(u16, char)>) -> Self {
        let named: Vec<u16> = plain
            .iter()
            .filter(|(_, typed)| typed.is_control())
            .map(|&(code, _)| code)
            .collect();
        let control = control
            .into_iter()
            .filter(|(code, _)| !named.contains(code))
            .collect();
        Self {
            plain,
            command,
            control,
        }
    }
}

/// The virtual key a chord presses for `key` with `modifiers` held.
///
/// A single printable character is pressed by the key that types it with
/// those modifiers, so the app receives what the user's own keyboard sends
/// for that shortcut: `cmd+a` presses the AZERTY A key (US Q), `ctrl+c` on
/// Russian the key whose Control layer types `^C`. Command reads the Command
/// layer, Control the Control layer, any other chord the plain one. When
/// several keys type the character, the US key is kept, then the one that
/// types it with no modifier (Turkish F types `^C` on its C and Ç keys). A character the layer has no
/// key for, a named key, and every key when the layout could not be read keep
/// the US position, as macOS does for a shortcut whose character is not on
/// the layout.
fn chord_key_code(
    key: &str,
    modifiers: &[&str],
    layout: Option<&KeyLayout>,
) -> anyhow::Result<u16> {
    let us = key_name_to_code(key);
    let name = canonical_key_name(key);
    let mut chars = name.chars();
    let (Some(character), None, Some(layout)) = (chars.next(), chars.next(), layout) else {
        return us;
    };
    if character.is_whitespace() || character.is_control() {
        return us;
    }
    let held = |code: u16| {
        modifiers.iter().any(|modifier| {
            modifier_key_code_and_flag(modifier).is_some_and(|(key, _)| key == code)
        })
    };
    let character = character.to_lowercase().next().unwrap_or(character);
    let (keys, control) = if held(55) {
        (&layout.command, None)
    } else if held(59) {
        let control = character
            .is_ascii_lowercase()
            .then(|| char::from(character as u8 & 0x1f));
        (&layout.control, control)
    } else {
        (&layout.plain, None)
    };
    let codes: Vec<u16> = keys
        .iter()
        .filter(|&&(_, typed)| {
            typed.to_lowercase().eq(std::iter::once(character)) || Some(typed) == control
        })
        .map(|&(code, _)| code)
        .collect();
    let keycap = codes.iter().copied().find(|code| {
        layout.plain.iter().any(|&(key, typed)| {
            key == *code && typed.to_lowercase().eq(std::iter::once(character))
        })
    });
    match us {
        Ok(code) if codes.contains(&code) => Ok(code),
        _ => keycap.or(codes.first().copied()).map_or(us, Ok),
    }
}

/// The virtual key a chord aimed at `pid` presses, on the current keyboard
/// layout. Every chord path resolves its key here. Screen Sharing forwards key
/// positions to a guest whose layout this host cannot read, so it keeps the US
/// positions its text path (`physical_key_for_char`) assumes too: the host
/// layout would press AZERTY's A key (US Q) for `cmd+a` on a US guest.
fn target_key_code(
    pid: impl Into<Option<i32>>,
    key: &str,
    modifiers: &[&str],
) -> anyhow::Result<u16> {
    let screen_sharing = pid.into().is_some_and(is_screen_sharing_pid);
    chord_key_code(key, modifiers, target_layout(screen_sharing).as_deref())
}

/// The virtual key `key` with `modifiers` presses on the current keyboard
/// layout, as `press_key` and `hotkey` resolve it for a local app.
pub fn layout_key_code(key: &str, modifiers: &[&str]) -> anyhow::Result<u16> {
    target_key_code(None, key, modifiers)
}

fn target_layout(screen_sharing: bool) -> Option<Arc<KeyLayout>> {
    (!screen_sharing).then(current_layout).flatten()
}

/// The main typing keys (the ANSI/ISO block). The keypad is left out: its
/// keys send navigation keys with Num Lock off and are not where a shortcut's
/// character is.
const TYPING_KEY_CODES: std::ops::RangeInclusive<u16> = 0..=50;

/// The current keyboard layout, or `None` when it has no Unicode layout data
/// or could not be read with the thread affinity Text Input Sources requires
/// (see [`super::input_source`]); chords then keep their US key positions.
///
/// In a process with a main run loop the layout is read once on the main
/// queue and cached until the selected or enabled input sources change.
fn current_layout() -> Option<Arc<KeyLayout>> {
    static CACHE: Mutex<Option<(u64, Option<Arc<KeyLayout>>)>> = Mutex::new(None);

    let generation = input_source::generation();
    if input_source::can_cache() {
        if let Some((cached, layout)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if *cached == generation {
                return layout.clone();
            }
        }
    }
    input_source::run(move || {
        let layout = read_current_layout().map(Arc::new);
        if input_source::can_cache() {
            *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((generation, layout.clone()));
        }
        layout
    })
    .flatten()
}

/// Read the current layout with `UCKeyTranslate`. Text Input Sources calls:
/// only through [`input_source::run`].
fn read_current_layout() -> Option<KeyLayout> {
    use core_foundation::base::{CFRelease, CFTypeRef};
    use core_foundation::data::{CFDataGetBytePtr, CFDataRef};
    use core_foundation::string::CFStringRef;

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
        fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
        fn TISGetInputSourceProperty(source: CFTypeRef, key: CFStringRef) -> CFTypeRef;
        fn LMGetKbdType() -> u8;
        fn UCKeyTranslate(
            layout: *const u8,
            key_code: u16,
            key_action: u16,
            modifier_key_state: u32,
            keyboard_type: u32,
            options: u32,
            dead_key_state: *mut u32,
            max_length: usize,
            actual_length: *mut usize,
            unicode: *mut u16,
        ) -> i32;
    }
    const KEY_ACTION_DISPLAY: u16 = 3;
    const NO_DEAD_KEYS: u32 = 1;
    // `cmdKey >> 8` and `controlKey >> 8`, the modifier states UCKeyTranslate takes.
    const COMMAND_STATE: u32 = 0x01;
    const CONTROL_STATE: u32 = 0x10;

    unsafe {
        let source = TISCopyCurrentKeyboardLayoutInputSource();
        if source.is_null() {
            return None;
        }
        let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        let layout = (!data.is_null()).then(|| {
            let bytes = CFDataGetBytePtr(data as CFDataRef);
            let keyboard_type = u32::from(LMGetKbdType());
            let layer = |state: u32| {
                TYPING_KEY_CODES
                    .filter_map(|code| {
                        let mut dead = 0u32;
                        let mut length = 0usize;
                        let mut unicode = [0u16; 4];
                        let status = UCKeyTranslate(
                            bytes,
                            code,
                            KEY_ACTION_DISPLAY,
                            state,
                            keyboard_type,
                            NO_DEAD_KEYS,
                            &mut dead,
                            unicode.len(),
                            &mut length,
                            unicode.as_mut_ptr(),
                        );
                        let mut typed =
                            char::decode_utf16(unicode[..length.min(4)].iter().copied());
                        match (status, typed.next(), typed.next()) {
                            (0, Some(Ok(character)), None) => Some((code, character)),
                            _ => None,
                        }
                    })
                    .collect()
            };
            KeyLayout::new(layer(0), layer(COMMAND_STATE), layer(CONTROL_STATE))
        });
        CFRelease(source);
        layout
    }
}

/// The error for a name `key_name_to_code` does not know, saying what to send
/// instead: a combination such as "shift+Right" is a key plus modifiers, and
/// a printable symbol is text.
fn unknown_key_message(key: &str) -> String {
    if key.chars().count() > 1 && key.contains('+') && !key.ends_with('+') {
        let parts: Vec<&str> = key.split('+').collect();
        let (modifiers, name) = parts.split_at(parts.len() - 1);
        return format!(
            "Unknown key name: {key}. A key name is one key, not a combination. Send the last \
             part as the key and the rest as modifiers, e.g. key:\"{}\" with modifiers:{:?}, \
             or use the hotkey tool with keys:{:?}.",
            name[0],
            modifiers
                .iter()
                .map(|m| m.to_lowercase())
                .collect::<Vec<_>>(),
            parts
        );
    }
    format!(
        "Unknown key name: {key}. Use a key name such as return, tab, escape, space, delete, \
         left, right, up, down, pageup, pagedown, home, end, f1..f12, or a single letter or \
         digit; to enter other characters (for example * or $) use type_text."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_graphics::event::CGEventType;

    #[test]
    fn common_key_spellings_resolve_to_one_key() {
        for (spelling, name) in [
            ("Page_Down", "pagedown"),
            ("page-down", "pagedown"),
            ("PgDn", "pagedown"),
            ("Page Up", "pageup"),
            ("ArrowLeft", "left"),
            ("left_arrow", "left"),
            ("Forward_Delete", "del"),
            ("ESC", "escape"),
            ("KP_Enter", "return"),
        ] {
            assert_eq!(
                super::key_name_to_code(spelling).unwrap(),
                super::key_name_to_code(name).unwrap(),
                "{spelling}"
            );
        }
        // Single characters are keys in their own right.
        assert_eq!(super::key_name_to_code("-").unwrap(), 27);
        assert!(super::key_name_to_code("asterisk").is_err());
    }

    #[test]
    fn unknown_key_errors_say_what_to_send_instead() {
        let combo = key_name_to_code("shift+Right").unwrap_err().to_string();
        assert!(combo.contains("modifiers"), "{combo}");
        assert!(combo.contains("hotkey"), "{combo}");
        assert!(combo.contains("key:\"Right\""), "{combo}");
        let symbol = key_name_to_code("asterisk").unwrap_err().to_string();
        assert!(symbol.contains("type_text"), "{symbol}");
        assert!(key_name_to_code("shift").is_ok());
    }

    /// cua-spacesd names the Meta key `super` (the cross-platform name), and
    /// the macOS driver dropped it: `cua do hotkey cmd+w` pressed a bare `w`.
    #[test]
    fn super_meta_and_win_are_command() {
        for name in ["cmd", "command", "super", "meta", "win", "Super"] {
            assert_eq!(
                modifier_key_code_and_flag(name),
                Some((55, CGEventFlags::CGEventFlagCommand)),
                "{name}"
            );
            assert_eq!(
                modifier_flags(&[name]),
                CGEventFlags::CGEventFlagCommand,
                "{name}"
            );
            assert_eq!(key_name_to_code(name).unwrap(), 55, "{name}");
        }
    }

    #[test]
    fn physical_text_uses_flags_changed_for_balanced_shift_transitions() {
        assert_eq!(
            physical_text_events('a').unwrap(),
            vec![
                PhysicalTextEvent {
                    key_code: 0,
                    key_down: true,
                    event_type: PhysicalEventType::KeyDown,
                    shift: false,
                    text: Some('a'),
                },
                PhysicalTextEvent {
                    key_code: 0,
                    key_down: false,
                    event_type: PhysicalEventType::KeyUp,
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
                    event_type: PhysicalEventType::FlagsChanged,
                    shift: true,
                    text: None,
                },
                PhysicalTextEvent {
                    key_code: 6,
                    key_down: true,
                    event_type: PhysicalEventType::KeyDown,
                    shift: true,
                    text: Some('Z'),
                },
                PhysicalTextEvent {
                    key_code: 6,
                    key_down: false,
                    event_type: PhysicalEventType::KeyUp,
                    shift: true,
                    text: Some('Z'),
                },
                PhysicalTextEvent {
                    key_code: 56,
                    key_down: false,
                    event_type: PhysicalEventType::FlagsChanged,
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
    fn bare_modifier_events_derive_type_and_flags_from_default_source() {
        let shift_down = create_bare_keyboard_event(SHIFT_KEY_CODE, true).unwrap();
        assert_eq!(
            shift_down.get_type() as u32,
            CGEventType::FlagsChanged as u32
        );
        assert!(
            shift_down
                .get_flags()
                .contains(CGEventFlags::CGEventFlagShift),
            "bare Shift down must derive the active Shift flag"
        );
        assert_eq!(
            shift_down
                .get_integer_value_field(core_graphics::event::EventField::EVENT_SOURCE_STATE_ID),
            CGEventSourceStateID::CombinedSessionState as i64,
            "NULL source must resolve to the default combined-session state, not HID state"
        );

        let shift_up = create_bare_keyboard_event(SHIFT_KEY_CODE, false).unwrap();
        assert_eq!(shift_up.get_type() as u32, CGEventType::FlagsChanged as u32);
        assert!(
            !shift_up
                .get_flags()
                .contains(CGEventFlags::CGEventFlagShift),
            "bare Shift up must derive cleared Shift state"
        );

        let z_down = create_bare_keyboard_event(6, true).unwrap();
        assert_eq!(z_down.get_type() as u32, CGEventType::KeyDown as u32);
        assert_eq!(
            z_down
                .get_integer_value_field(core_graphics::event::EventField::KEYBOARD_EVENT_KEYCODE),
            6
        );
    }

    #[test]
    fn bare_command_chord_orders_modifier_base_and_reverse_release() {
        let command = (55, CGEventFlags::CGEventFlagCommand);
        assert_eq!(
            bare_chord_transitions(9, &[command]),
            vec![
                (55, true, CGEventFlags::CGEventFlagCommand),
                (9, true, CGEventFlags::CGEventFlagCommand),
                (9, false, CGEventFlags::CGEventFlagCommand),
                (55, false, CGEventFlags::CGEventFlagNull),
            ]
        );
    }

    /// The default source derives a modifier's own flag from its keycode but
    /// has nothing to derive from for the base key, which is what turned
    /// `cmd+a` into a literal `a` on the foreground rung.
    #[test]
    fn bare_base_key_carries_the_chord_flags_the_default_source_cannot_derive() {
        let command_down = create_bare_keyboard_event(55, true).unwrap();
        assert_eq!(
            command_down.get_type() as u32,
            CGEventType::FlagsChanged as u32
        );
        assert!(
            command_down
                .get_flags()
                .contains(CGEventFlags::CGEventFlagCommand),
            "bare Command down must derive the active Command flag"
        );

        let bare_base = create_bare_keyboard_event(0, true).unwrap();
        assert!(
            !bare_base
                .get_flags()
                .contains(CGEventFlags::CGEventFlagCommand),
            "the default source cannot derive Command for a non-modifier keycode"
        );

        let (code, down, flags) =
            bare_chord_transitions(0, &[(55, CGEventFlags::CGEventFlagCommand)])[1];
        let carried = create_bare_keyboard_event(code, down).unwrap();
        carried.set_flags(flags);
        assert!(
            carried
                .get_flags()
                .contains(CGEventFlags::CGEventFlagCommand),
            "the chord's base key must carry Command"
        );
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
    fn typed_newlines_and_tabs_are_real_return_and_tab_keys() {
        let return_code = key_name_to_code("return").unwrap();
        assert_eq!(control_key_for_char('\n'), Some(return_code));
        assert_eq!(control_key_for_char('\r'), Some(return_code));
        assert_eq!(
            control_key_for_char('\t'),
            Some(key_name_to_code("tab").unwrap())
        );
        for ch in ['a', ' ', '/', ':', 'é'] {
            assert_eq!(control_key_for_char(ch), None, "{ch:?}");
        }
    }

    /// A layout from the characters its keys 0..=50 type with no modifier,
    /// Command and Control, as `UCKeyTranslate` reads them on macOS 26
    /// (dumped from the installed layouts; `\0` is a key that types nothing).
    fn layout([plain, command, control]: [&str; 3]) -> KeyLayout {
        let keys = |row: &str| -> Vec<(u16, char)> {
            assert_eq!(row.chars().count(), 51);
            (0u16..)
                .zip(row.chars())
                .filter(|&(_, typed)| typed != '\0')
                .collect()
        };
        KeyLayout::new(keys(plain), keys(command), keys(control))
    }

    /// ABC: plain, Command and Control layers.
    const US: [&str; 3] = [
        "asdfhgzxcv§bqweryt123465=97-80]ou[ip\rlj'k;\\,/nm.\t `",
        "asdfhgzxcv§bqweryt123465=97-80]ou[ip\rlj'k;\\,/nm.\t `",
        "\x01\x13\x04\x06\x08\x07\x1a\x18\x03\x160\x02\x11\x17\x05\x12\x19\x14123465=97\x1f80\x1d\x0f\x15\x1b\t\x10\r\x0c\n'\x0b;\x1c,/\x0e\r.\t `",
    ];
    /// French: plain, Command and Control layers.
    const FRENCH: [&str; 3] = [
        "qsdfhgwxcv@bazeryt&é\"'§(-çè)!à$ou^ip\rljùkm`;=n,:\t <",
        "qsdfhgwxcv@bazeryt&é\"'§(-çè)!à$ou^ip\rljùkm`;=n,:\t <",
        "\x11\x13\x04\x06\x08\x07\x17\x18\x03\x160\x02\x01\x1a\x05\x12\x19\x141234\x1d5\x1f\x1c7\x1b8\0\x1d\x0f\x15\x1e\t\x10\r\x0c\nù\x0b\r\x1c;=\x0e,:\t <",
    ];
    /// German: plain, Command and Control layers.
    const GERMAN: [&str; 3] = [
        "asdfhgyxcv^bqwerzt123465´97ß80+ouüip\rljäkö#,-nm.\t <",
        "asdfhgyxcv^bqwerzt123465´97ß80+ouüip\rljäkö#,-nm.\t <",
        "\x01\x13\x04\x06\x08\x07\x19\x18\x03\x16\x1e\x02\x11\x17\x05\x12\x1a\x14123465´97ß80+\x0f\x15\x1d\t\x10\r\x0c\n\x1b\x0b\x1c#,-\x0e\r.\t <",
    ];
    /// Russian: plain, Command and Control layers.
    const RUSSIAN: [&str; 3] = [
        "фыварпячсм>ийцукне123465=97-80ъщгхшз\rдоэлжёб/тью\t ]",
        "asdfhgzxcv§bqweryt123465=97-80]ou[ip\rlj'k;\\,/nm.\t `",
        "\x01\x13\x04\x06\x08\x07\x1a\x18\x03\x160\x02\x11\x17\x05\x12\x19\x14123465=97\x1f80\x1d\x0f\x15\x1b\t\x10\r\x0c\n'\x0b;\x1c,/\x0e\r.\t `",
    ];
    /// Dvorak – QWERTY ⌘: plain, Command and Control layers.
    const DVORAK_QWERTY_CMD: [&str; 3] = [
        "aoeudi;qjk§x',.pfy123465]97[80=rg/cl\rnh-ts\\wzbmv\t `",
        "asdfhgzxcv§bqweryt123465=97-80]ou[ip\rlj'k;\\,/nm.\t `",
        "\x01\x13\x04\x06\x08\x07\x1a\x18\x03\x160\x02\x11\x17\x05\x12\x19\x14123465=97\x1f80\x1d\x0f\x15\x1b\t\x10\r\x0c\n'\x0b;\x1c,/\x0e\r.\t `",
    ];
    /// Hungarian: plain, Command and Control layers.
    const HUNGARIAN: [&str; 3] = [
        "asdfhgyxcv0bqwerzt123465ó97ü8öúouőip\rljákéű,-nm.\t í",
        "asdfhgyxcv0bqwerzt123465ó97ü8öúouőip\rljákéű,-nm.\t í",
        "\x01\x13\x04\x06\x08\x07\x19\x18\x03\x160\x02\x11\x17\x05\x12\x1a\x14123465=97\x1f80\x1d\x0f\x15\x1b\t\x10\r\x0c\n'\x0b;\x1c,/\x0e\r.\t `",
    ];
    /// Lithuanian – QWERTY: plain, Command and Control layers.
    const LITHUANIAN: [&str; 3] = [
        "asdfhgzxcv§bqwerytąčęėšįž„ų-ū“]ou[ip\rlj'k;\\,/nm.\t `",
        "asdfhgzxcv§bqweryt123465=97-80]ou[ip\rlj'k;\\,/nm.\t `",
        "asdfhgzxcv§bqweryt123465=97-80]ou[ip\rlj'k;\\,/nm.\t `",
    ];

    fn code(key: &str, modifiers: &[&str], layout: [&str; 3]) -> u16 {
        chord_key_code(key, modifiers, Some(&self::layout(layout))).unwrap()
    }

    /// HW-106: `cmd+a` on an AZERTY Mac pressed the US A key, which types
    /// `q` there, and Slack quit.
    #[test]
    fn azerty_shortcuts_press_the_key_that_types_their_letter() {
        assert_eq!(code("a", &["cmd"], FRENCH), 12);
        assert_eq!(code("q", &["cmd"], FRENCH), 0);
        assert_eq!(code("w", &["cmd"], FRENCH), 6);
        assert_eq!(code("z", &["cmd", "shift"], FRENCH), 13);
        assert_eq!(code("A", &["cmd"], FRENCH), 12);
        assert_eq!(code("a", &["ctrl"], FRENCH), 12);
        // Return types ^M with Control held too; the M key is pressed.
        assert_eq!(code("m", &["ctrl"], FRENCH), 41);
        // Letters that sit where US has them keep the US key.
        assert_eq!(code("c", &["cmd"], FRENCH), 8);
        // Symbols too: cmd+= zooms in rather than pressing AZERTY `-`.
        assert_eq!(code("=", &["cmd"], FRENCH), 44);
        assert_eq!(code("-", &["cmd"], FRENCH), 24);
    }

    #[test]
    fn qwertz_swaps_y_and_z() {
        assert_eq!(code("z", &["cmd"], GERMAN), 16);
        assert_eq!(code("y", &["ctrl"], GERMAN), 6);
        assert_eq!(code("ß", &[], GERMAN), 27);
    }

    /// Russian and Dvorak - QWERTY ⌘ type QWERTY with Command or Control
    /// held, so their shortcuts are on the QWERTY keys.
    #[test]
    fn chords_read_the_layer_of_their_modifiers() {
        assert_eq!(code("c", &["cmd"], RUSSIAN), 8);
        assert_eq!(code("c", &["ctrl"], RUSSIAN), 8);
        assert_eq!(code("c", &["cmd"], DVORAK_QWERTY_CMD), 8);
        assert_eq!(code("c", &["ctrl"], DVORAK_QWERTY_CMD), 8);
        assert_eq!(code("c", &["shift"], DVORAK_QWERTY_CMD), 34);
        // Lithuanian types plain letters with Control held.
        assert_eq!(code("c", &["ctrl"], LITHUANIAN), 8);
    }

    /// Turkish F types `^C` on its C key (9) and its Ç key (11).
    #[test]
    fn of_several_keys_the_one_that_types_the_letter_is_pressed() {
        // The Ç key first, as if it came first in the layout.
        let turkish_f = KeyLayout::new(
            vec![(8, 'ü'), (9, 'c'), (11, 'ç')],
            vec![(8, 'ü'), (9, 'c'), (11, 'ç')],
            vec![(11, '\x03'), (8, '\x16'), (9, '\x03')],
        );
        assert_eq!(chord_key_code("c", &["ctrl"], Some(&turkish_f)).unwrap(), 9);
    }

    /// Digits are where the layer types them: Hungarian has 0 left of 1.
    /// AZERTY types `&` on the 1 key with Command held, so the 1 key stays.
    #[test]
    fn digits_are_pressed_where_the_layer_types_them() {
        assert_eq!(code("0", &["cmd"], HUNGARIAN), 10);
        assert_eq!(code("1", &["cmd"], FRENCH), 18);
        assert_eq!(code("1", &["ctrl"], FRENCH), 18);
        assert_eq!(code("1", &["cmd"], LITHUANIAN), 18);
        assert_eq!(code("5", &["cmd"], US), 23);
    }

    /// Like macOS for a shortcut whose character is not on the layout.
    #[test]
    fn a_character_the_layer_lacks_keeps_its_us_key() {
        assert_eq!(code("[", &["cmd"], FRENCH), 33);
        assert_eq!(code("c", &[], RUSSIAN), 8);
    }

    #[test]
    fn named_keys_keep_their_positions() {
        for (key, us) in [
            ("return", 36),
            ("left", 123),
            ("f5", 96),
            ("space", 49),
            ("tab", 48),
        ] {
            assert_eq!(code(key, &["ctrl"], FRENCH), us, "{key}");
        }
        assert_eq!(chord_key_code("a", &["cmd"], None).unwrap(), 0);
        assert!(chord_key_code("asterisk", &[], Some(&layout(FRENCH)))
            .unwrap_err()
            .to_string()
            .contains("Unknown key name"));
    }

    /// A French host driving a US guest through Screen Sharing: `cmd+a` must
    /// press the US A key, not the host's AZERTY A key (US Q).
    #[test]
    fn screen_sharing_chords_keep_us_positions() {
        assert!(target_layout(true).is_none());
        assert_eq!(
            chord_key_code("a", &["cmd"], target_layout(true).as_deref()).unwrap(),
            0
        );
    }

    /// Reads the host's real layout through Carbon: per-user state, so not in
    /// plain `cargo test`.
    #[test]
    #[ignore = "reads the host keyboard layout"]
    fn the_current_layout_types_its_letters() {
        let layout = current_layout().expect("current layout has Unicode data");
        for letter in 'a'..='z' {
            if layout.plain.iter().all(|&(_, typed)| typed != letter) {
                continue;
            }
            let code = chord_key_code(&letter.to_string(), &[], Some(&*layout)).unwrap();
            assert!(layout.plain.contains(&(code, letter)), "{letter}");
        }
    }

    #[test]
    fn screen_sharing_detection_is_exact_and_case_sensitive() {
        assert!(is_screen_sharing_bundle_id("com.apple.ScreenSharing"));
        assert!(!is_screen_sharing_bundle_id("com.apple.screensharing"));
        assert!(!is_screen_sharing_bundle_id("com.microsoft.rdc.macos"));
    }
}

#[cfg(test)]
mod chunk_tests {
    use super::*;

    #[test]
    fn short_text_is_one_pair() {
        assert_eq!(plan_chunks("hello"), vec![TypingChunk::Text("hello")]);
    }

    #[test]
    fn controls_split_runs_and_stay_real_keys() {
        assert_eq!(
            plan_chunks("a\tb\n"),
            vec![
                TypingChunk::Text("a"),
                TypingChunk::Control('\t'),
                TypingChunk::Text("b"),
                TypingChunk::Control('\n'),
            ]
        );
    }

    #[test]
    fn long_text_chunks_never_exceed_the_unit_limit_and_round_trip() {
        let text = "x".repeat(100) + "é😀日本語" + &"y".repeat(33);
        let chunks = plan_chunks(&text);
        let mut joined = String::new();
        for c in &chunks {
            let TypingChunk::Text(t) = c else { panic!() };
            assert!(t.encode_utf16().count() <= MAX_UNICODE_UNITS_PER_EVENT);
            joined.push_str(t);
        }
        assert_eq!(joined, text);
        assert!(chunks.len() < text.chars().count() / 8);
    }

    #[test]
    fn astral_characters_are_not_split() {
        let text = "😀".repeat(9); // 18 units
        let chunks = plan_chunks(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], TypingChunk::Text(&"😀".repeat(8)));
    }

    #[test]
    fn empty_text_is_no_chunks() {
        assert!(plan_chunks("").is_empty());
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// Micro-benchmark: wall time to post a 100-character string to `pid`,
    /// paced (the old 30 ms default) versus batched. Run with
    /// `CUA_BENCH_PID=<pid> cargo test -p platform-macos bench_type_text_posting -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_type_text_posting() {
        let pid: i32 = std::env::var("CUA_BENCH_PID")
            .ok()
            .and_then(|v| v.parse().ok())
            .expect("set CUA_BENCH_PID to a running app's pid");
        let text = "The quick brown fox jumps over the lazy dog. 0123456789 hello there, world! Cua Driver type_text bench.";
        assert!(text.chars().count() >= 100);
        for (label, delay) in [
            ("paced (before, delay_ms=30)", 30u64),
            ("batched (after, delay_ms=0)", 0),
        ] {
            let mut ms: Vec<f64> = (0..9)
                .map(|_| {
                    let t = std::time::Instant::now();
                    type_text_with_delay(pid, text, delay).unwrap();
                    t.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
            println!(
                "{label}: p50 {:.0} ms over {} chars",
                ms[ms.len() / 2],
                text.chars().count()
            );
        }
    }
}
