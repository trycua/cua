//! Keystroke coalescing.
//!
//! Raw key events are noisy and privacy-sensitive. The coalescer turns a stream
//! of individual key presses into the higher-level [`HumanEvent`]s a trajectory
//! actually wants:
//!
//! * Runs of plain text-producing keys (letters, digits, punctuation, space,
//!   plus in-line editing with Backspace) collapse into a single
//!   [`HumanEvent::Text`], whose literal characters are **redacted** unless raw
//!   capture was explicitly enabled.
//! * Command keystrokes (Enter, Tab, Esc, arrows, F-keys) and any keystroke
//!   held with Ctrl/Alt/Meta flush the pending text run and emit a discrete
//!   [`HumanEvent::Key`] with a semantic name.
//!
//! The coalescer is pure and deterministic: the platform hook feeds it
//! `(KeyInput, t_ms)` and a flush trigger (focus loss / idle timeout), and it
//! returns events to persist. No OS calls, fully unit-testable.

use crate::event::{redact, HumanEvent, Mods};

/// Idle gap (ms) after which a pending text run is flushed even without a
/// command key — so a pause between typing and the next action delimits steps.
pub const DEFAULT_IDLE_FLUSH_MS: u64 = 1500;

/// A normalized key press handed to the coalescer by the platform layer.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyInput {
    /// The character this key produces *as text*, if any (after layout/shift).
    /// `None` for non-text keys (Enter, F2, arrows, …).
    pub text_char: Option<char>,
    /// Semantic name for non-text/command keys: "Enter", "Tab", "Backspace",
    /// "ArrowLeft", "F2", "a" (for Ctrl+A), etc.
    pub name: String,
    pub mods: Mods,
}

/// Stateful keystroke coalescer. One per recording session.
pub struct Coalescer {
    capture_raw: bool,
    idle_flush_ms: u64,
    /// Accumulated literal text of the in-progress run (kept in memory only;
    /// emitted redacted unless `capture_raw`).
    buf: String,
    /// Timestamp of the first keystroke in the current run (the event's t_ms).
    run_start_ms: Option<u64>,
    /// Timestamp of the most recent keystroke, for idle-flush accounting.
    last_key_ms: u64,
}

impl Coalescer {
    pub fn new(capture_raw: bool) -> Self {
        Self {
            capture_raw,
            idle_flush_ms: DEFAULT_IDLE_FLUSH_MS,
            buf: String::new(),
            run_start_ms: None,
            last_key_ms: 0,
        }
    }

    pub fn with_idle_flush(mut self, ms: u64) -> Self {
        self.idle_flush_ms = ms;
        self
    }

    /// Feed one key press. Returns zero or more events to persist (a text flush
    /// may precede a command key in the same call).
    pub fn push(&mut self, key: KeyInput, t_ms: u64) -> Vec<HumanEvent> {
        let mut out = Vec::new();

        // Idle timeout: a long pause since the last keystroke ends the run.
        if self.run_start_ms.is_some() && t_ms.saturating_sub(self.last_key_ms) >= self.idle_flush_ms
        {
            if let Some(ev) = self.flush() {
                out.push(ev);
            }
        }

        let is_command = key.mods.is_command();
        match (key.text_char, is_command) {
            // Plain text character with no command modifier -> accumulate.
            (Some(c), false) => {
                if self.run_start_ms.is_none() {
                    self.run_start_ms = Some(t_ms);
                }
                self.buf.push(c);
                self.last_key_ms = t_ms;
            }
            // Backspace mid-run: edit the buffer instead of emitting a key, so
            // small corrections stay inside one Text event.
            (None, false) if key.name == "Backspace" && self.run_start_ms.is_some() => {
                self.buf.pop();
                self.last_key_ms = t_ms;
            }
            // Anything else (command keys, navigation, function keys, or a
            // text char held with Ctrl/Alt/Meta) flushes then emits a Key.
            _ => {
                if let Some(ev) = self.flush() {
                    out.push(ev);
                }
                out.push(HumanEvent::Key {
                    name: hotkey_name(&key),
                    mods: key.mods,
                    t_ms,
                });
            }
        }
        out
    }

    /// Flush any pending text run into a [`HumanEvent::Text`]. Returns `None`
    /// when the buffer is empty. Call on focus loss / stop.
    pub fn flush(&mut self) -> Option<HumanEvent> {
        let start = self.run_start_ms.take()?;
        if self.buf.is_empty() {
            return None;
        }
        let text = std::mem::take(&mut self.buf);
        let char_count = text.chars().count();
        Some(HumanEvent::Text {
            redacted: redact(&text),
            raw: if self.capture_raw { Some(text) } else { None },
            char_count,
            t_ms: start,
        })
    }
}

/// Build a semantic name for a command keystroke, including a modifier prefix
/// for hotkeys (`Ctrl+C`, `Ctrl+Shift+T`). Plain navigation keys keep their
/// bare name (`Enter`, `Tab`).
fn hotkey_name(key: &KeyInput) -> String {
    if key.mods.is_command() {
        format!("{}{}", key.mods.prefix(), key.name)
    } else {
        key.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Button;

    fn ch(c: char) -> KeyInput {
        KeyInput { text_char: Some(c), name: c.to_string(), mods: Mods::default() }
    }
    fn named(name: &str) -> KeyInput {
        KeyInput { text_char: None, name: name.into(), mods: Mods::default() }
    }

    #[test]
    fn plain_typing_coalesces_into_one_redacted_text() {
        let mut c = Coalescer::new(false);
        let mut events = Vec::new();
        for (i, ch_) in "hello".chars().enumerate() {
            events.extend(c.push(ch(ch_), i as u64 * 10));
        }
        assert!(events.is_empty(), "no events until flush/command");
        let ev = c.flush().unwrap();
        match ev {
            HumanEvent::Text { redacted, raw, char_count, t_ms } => {
                assert_eq!(char_count, 5);
                assert_eq!(t_ms, 0, "stamped at run start");
                assert!(raw.is_none(), "redacted by default");
                assert!(!redacted.contains("hello"));
                assert!(redacted.contains("5 chars"));
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn raw_capture_preserves_text() {
        let mut c = Coalescer::new(true);
        for ch_ in "hi".chars() {
            c.push(ch(ch_), 0);
        }
        let ev = c.flush().unwrap();
        match ev {
            HumanEvent::Text { raw, .. } => assert_eq!(raw.as_deref(), Some("hi")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn enter_flushes_text_then_emits_key() {
        let mut c = Coalescer::new(true);
        let mut out = Vec::new();
        for ch_ in "abc".chars() {
            out.extend(c.push(ch(ch_), 0));
        }
        out.extend(c.push(named("Enter"), 100));
        assert_eq!(out.len(), 2);
        match &out[0] {
            HumanEvent::Text { raw, .. } => assert_eq!(raw.as_deref(), Some("abc")),
            o => panic!("{o:?}"),
        }
        match &out[1] {
            HumanEvent::Key { name, .. } => assert_eq!(name, "Enter"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn ctrl_c_is_hotkey_not_text() {
        let mut c = Coalescer::new(true);
        let key = KeyInput {
            text_char: Some('c'),
            name: "c".into(),
            mods: Mods { ctrl: true, ..Default::default() },
        };
        let out = c.push(key, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            HumanEvent::Key { name, .. } => assert_eq!(name, "Ctrl+c"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn backspace_edits_run_in_place() {
        let mut c = Coalescer::new(true);
        c.push(ch('a'), 0);
        c.push(ch('b'), 1);
        c.push(named("Backspace"), 2);
        c.push(ch('c'), 3);
        let ev = c.flush().unwrap();
        match ev {
            HumanEvent::Text { raw, char_count, .. } => {
                assert_eq!(raw.as_deref(), Some("ac"));
                assert_eq!(char_count, 2);
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn idle_gap_flushes_run() {
        let mut c = Coalescer::new(true).with_idle_flush(1000);
        c.push(ch('a'), 0);
        // 1200 ms gap -> the next key first flushes "a", then starts a new run.
        let out = c.push(ch('b'), 1200);
        assert_eq!(out.len(), 1);
        match &out[0] {
            HumanEvent::Text { raw, .. } => assert_eq!(raw.as_deref(), Some("a")),
            o => panic!("{o:?}"),
        }
        // "b" is now pending in a fresh run.
        let ev = c.flush().unwrap();
        match ev {
            HumanEvent::Text { raw, t_ms, .. } => {
                assert_eq!(raw.as_deref(), Some("b"));
                assert_eq!(t_ms, 1200);
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn backspace_with_no_run_is_a_key() {
        let mut c = Coalescer::new(true);
        let out = c.push(named("Backspace"), 0);
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], HumanEvent::Key { name, .. } if name == "Backspace"));
    }

    // Touch Button so the import is always exercised across cfgs.
    #[test]
    fn button_is_usable() {
        assert_ne!(Button::Left, Button::Right);
    }
}
