//! Host-chosen key events for desktop-scope typed text (macOS only).
//!
//! The macOS adapter types desktop-scope text as Unicode payloads on key code
//! 0. An event tap that rebuilds keyboard events from key code and flags, such
//! as Karabiner-Elements' CGEventTap fallback, drops the payload, so every
//! character arrives as `a`. A host with such a tap can select physical
//! US-keyboard key presses at trusted launch through the daemon environment:
//!
//! - [`TEXT_KEY_EVENTS_ENV`] (`CUA_DRIVER_TEXT_KEY_EVENTS`): `unicode` or
//!   `physical`.
//!
//! Unset or empty keeps `unicode`. Any other value is an error that
//! desktop-scope `type_text` reports instead of typing. The value is read from
//! the daemon process environment only; no tool argument can change it. Linux
//! and Windows do not read it. See `Skills/cua-driver/EMBEDDING.md`.

/// Daemon environment variable that selects [`TextKeyEvents`].
pub const TEXT_KEY_EVENTS_ENV: &str = "CUA_DRIVER_TEXT_KEY_EVENTS";

/// The host's choice of key events for desktop-scope typed text. The macOS
/// `type_text` tool resolves it once from [`text_key_events_from_env`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKeyEvents {
    /// Each character is a Unicode payload on key code 0. Types any character
    /// regardless of keyboard layout.
    Unicode,
    /// Each character is the physical key, plus Shift, that types it on the US
    /// layout. Survives event taps that drop the Unicode payload; text with a
    /// character that has no US key is refused.
    Physical,
}

impl TextKeyEvents {
    /// The environment value that selects this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unicode => "unicode",
            Self::Physical => "physical",
        }
    }
}

/// Resolve the mode from a raw environment value. Pure, so it can be tested
/// without touching the process environment.
pub fn text_key_events_from_raw(raw: Option<&str>) -> Result<TextKeyEvents, String> {
    let value = raw.map(str::trim).unwrap_or_default();
    if value.is_empty() || value.eq_ignore_ascii_case(TextKeyEvents::Unicode.as_str()) {
        Ok(TextKeyEvents::Unicode)
    } else if value.eq_ignore_ascii_case(TextKeyEvents::Physical.as_str()) {
        Ok(TextKeyEvents::Physical)
    } else {
        Err(format!(
            "{TEXT_KEY_EVENTS_ENV}={value:?} is not a supported value; use \"unicode\" or \"physical\""
        ))
    }
}

/// Resolve the mode from the daemon environment.
pub fn text_key_events_from_env() -> Result<TextKeyEvents, String> {
    match std::env::var(TEXT_KEY_EVENTS_ENV) {
        Ok(raw) => text_key_events_from_raw(Some(&raw)),
        Err(std::env::VarError::NotPresent) => text_key_events_from_raw(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(format!("{TEXT_KEY_EVENTS_ENV} is not valid Unicode"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_or_empty_keeps_unicode() {
        assert_eq!(text_key_events_from_raw(None), Ok(TextKeyEvents::Unicode));
        assert_eq!(
            text_key_events_from_raw(Some("")),
            Ok(TextKeyEvents::Unicode)
        );
        assert_eq!(
            text_key_events_from_raw(Some("  ")),
            Ok(TextKeyEvents::Unicode)
        );
    }

    #[test]
    fn named_modes_are_honored() {
        assert_eq!(
            text_key_events_from_raw(Some("physical")),
            Ok(TextKeyEvents::Physical)
        );
        assert_eq!(
            text_key_events_from_raw(Some(" Physical\n")),
            Ok(TextKeyEvents::Physical)
        );
        assert_eq!(
            text_key_events_from_raw(Some("unicode")),
            Ok(TextKeyEvents::Unicode)
        );
    }

    #[test]
    fn unknown_values_are_errors_not_the_default() {
        assert_eq!(
            text_key_events_from_raw(Some("us")),
            Err(
                "CUA_DRIVER_TEXT_KEY_EVENTS=\"us\" is not a supported value; use \"unicode\" or \"physical\""
                    .to_owned()
            )
        );
    }
}
