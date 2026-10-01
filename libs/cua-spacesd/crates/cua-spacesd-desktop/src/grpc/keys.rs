// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The contract's typed `Key` enum mapped to the key names the platform
//! input paths understand.
//!
//! Names are the portable vocabulary used by cua-driver tools ("return",
//! "ctrl", "pageup", "f5", single characters). Keys with no portable name map
//! to an explicit X keysym (`keysym:0x…`) on Linux; other platforms report
//! them as unsupported.

use cua_proto::env::v1::{key_input, Key, KeyInput};

/// Portable name for a named key, or `None` for `KEY_UNSPECIFIED`.
pub fn key_name(key: Key) -> Option<String> {
    use Key::*;
    let name: &str = match key {
        Unspecified => return None,
        Shift | ShiftLeft => "shift",
        ShiftRight => "keysym:0xffe2",
        Control | ControlLeft => "ctrl",
        ControlRight => "keysym:0xffe4",
        Alt | AltLeft => "alt",
        AltRight => "keysym:0xffea",
        Meta | MetaLeft => "super",
        MetaRight => "keysym:0xffec",
        Fn => "keysym:0x1008ff2b",
        CapsLock => "capslock",
        Enter => "return",
        Tab => "tab",
        Space => "space",
        Backspace => "backspace",
        Delete => "delete",
        Escape => "escape",
        Insert => "insert",
        Home => "home",
        End => "end",
        PageUp => "pageup",
        PageDown => "pagedown",
        ArrowUp => "up",
        ArrowDown => "down",
        ArrowLeft => "left",
        ArrowRight => "right",
        ContextMenu => "keysym:0xff67",
        Help => "keysym:0xff6a",
        F1 | F2 | F3 | F4 | F5 | F6 | F7 | F8 | F9 | F10 | F11 | F12 | F13 | F14 | F15 | F16
        | F17 | F18 | F19 | F20 | F21 | F22 | F23 | F24 => {
            return Some(format!("f{}", key as i32 - F1 as i32 + 1));
        }
        A | B | C | D | E | F | G | H | I | J | K | L | M | N | O | P | Q | R | S | T | U | V
        | W | X | Y | Z => {
            return Some(char::from(b'a' + (key as i32 - A as i32) as u8).to_string());
        }
        Digit0 | Digit1 | Digit2 | Digit3 | Digit4 | Digit5 | Digit6 | Digit7 | Digit8 | Digit9 => {
            return Some(char::from(b'0' + (key as i32 - Digit0 as i32) as u8).to_string());
        }
        Minus => "-",
        Equal => "=",
        BracketLeft => "[",
        BracketRight => "]",
        Backslash => "\\",
        Semicolon => ";",
        Quote => "'",
        Backquote => "`",
        Comma => ",",
        Period => ".",
        Slash => "/",
        IntlBackslash => "keysym:0x3c",
        Numpad0 | Numpad1 | Numpad2 | Numpad3 | Numpad4 | Numpad5 | Numpad6 | Numpad7 | Numpad8
        | Numpad9 => {
            return Some(format!(
                "keysym:0x{:x}",
                0xffb0 + (key as i32 - Numpad0 as i32)
            ));
        }
        NumpadAdd => "keysym:0xffab",
        NumpadSubtract => "keysym:0xffad",
        NumpadMultiply => "keysym:0xffaa",
        NumpadDivide => "keysym:0xffaf",
        NumpadDecimal => "keysym:0xffae",
        NumpadEnter => "keysym:0xff8d",
        NumpadEqual => "keysym:0xffbd",
        NumLock => "keysym:0xff7f",
        PrintScreen => "keysym:0xff61",
        ScrollLock => "keysym:0xff14",
        Pause => "keysym:0xff13",
        MediaPlayPause => "keysym:0x1008ff14",
        MediaStop => "keysym:0x1008ff15",
        MediaNext => "keysym:0x1008ff17",
        MediaPrevious => "keysym:0x1008ff16",
        VolumeUp => "keysym:0x1008ff13",
        VolumeDown => "keysym:0x1008ff11",
        VolumeMute => "keysym:0x1008ff12",
        BrightnessUp => "keysym:0x1008ff02",
        BrightnessDown => "keysym:0x1008ff03",
        Eject => "keysym:0x1008ff2c",
        Power => "keysym:0x1008ff2a",
        Sleep => "keysym:0x1008ff2f",
        BrowserBack => "keysym:0x1008ff26",
        BrowserForward => "keysym:0x1008ff27",
        BrowserRefresh => "keysym:0x1008ff29",
        BrowserHome => "keysym:0x1008ff18",
        BrowserSearch => "keysym:0x1008ff1b",
        LaunchMail => "keysym:0x1008ff19",
        LaunchApp1 => "keysym:0x1008ff5d",
        LaunchApp2 => "keysym:0x1008ff5e",
    };
    Some(name.to_owned())
}

/// Name for a `KeyInput` (named key or a single character).
pub fn key_input_name(input: &KeyInput) -> Option<String> {
    match input.key.as_ref()? {
        key_input::Key::Named(value) => Key::try_from(*value).ok().and_then(key_name),
        key_input::Key::Character(character) => {
            let mut chars = character.chars();
            match (chars.next(), chars.next()) {
                (Some(single), None) => Some(single.to_string()),
                _ => None,
            }
        }
    }
}

/// Names for a list of modifier keys (unknown values are skipped).
pub fn modifier_names(modifiers: &[i32]) -> Vec<String> {
    modifiers
        .iter()
        .filter_map(|value| Key::try_from(*value).ok())
        .filter_map(key_name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_has_a_name() {
        for value in 1..=300 {
            if let Ok(key) = Key::try_from(value) {
                assert!(key_name(key).is_some(), "{key:?}");
            }
        }
        assert_eq!(key_name(Key::F5).as_deref(), Some("f5"));
        assert_eq!(key_name(Key::Q).as_deref(), Some("q"));
        assert_eq!(key_name(Key::Digit7).as_deref(), Some("7"));
        assert_eq!(key_name(Key::Numpad3).as_deref(), Some("keysym:0xffb3"));
        assert_eq!(key_name(Key::Unspecified), None);
    }

    #[test]
    fn key_inputs_accept_named_keys_and_single_characters() {
        let named = KeyInput {
            key: Some(key_input::Key::Named(Key::Enter as i32)),
        };
        assert_eq!(key_input_name(&named).as_deref(), Some("return"));
        let character = KeyInput {
            key: Some(key_input::Key::Character("é".into())),
        };
        assert_eq!(key_input_name(&character).as_deref(), Some("é"));
        let many = KeyInput {
            key: Some(key_input::Key::Character("ab".into())),
        };
        assert_eq!(key_input_name(&many), None);
        assert_eq!(
            modifier_names(&[Key::Control as i32, Key::Shift as i32]),
            vec!["ctrl", "shift"]
        );
    }
}
