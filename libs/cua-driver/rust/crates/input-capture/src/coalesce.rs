//! Convert key presses into privacy-safe text and command observations.

use crate::event::{HumanEvent, Modifiers};

pub const DEFAULT_IDLE_FLUSH_MS: u64 = 1500;

#[derive(Debug, Clone, PartialEq)]
pub struct KeyInput {
    pub text_char: Option<char>,
    pub key: String,
    pub modifiers: Modifiers,
}

pub struct Coalescer {
    idle_flush_ms: u64,
    char_count: usize,
    run_start_ms: Option<u64>,
    last_key_ms: u64,
}

impl Coalescer {
    pub fn new() -> Self {
        Self {
            idle_flush_ms: DEFAULT_IDLE_FLUSH_MS,
            char_count: 0,
            run_start_ms: None,
            last_key_ms: 0,
        }
    }

    #[cfg(test)]
    fn with_idle_flush(mut self, ms: u64) -> Self {
        self.idle_flush_ms = ms;
        self
    }

    pub fn push(&mut self, input: KeyInput, t_ms: u64) -> Vec<HumanEvent> {
        let mut events = Vec::new();
        if self.run_start_ms.is_some()
            && t_ms.saturating_sub(self.last_key_ms) >= self.idle_flush_ms
        {
            if let Some(event) = self.flush() {
                events.push(event);
            }
        }

        match (input.text_char, input.modifiers.is_command()) {
            (Some(_), false) => {
                self.run_start_ms.get_or_insert(t_ms);
                self.char_count += 1;
                self.last_key_ms = t_ms;
            }
            (None, false) if input.key == "Backspace" && self.run_start_ms.is_some() => {
                self.char_count = self.char_count.saturating_sub(1);
                self.last_key_ms = t_ms;
            }
            _ => {
                if let Some(event) = self.flush() {
                    events.push(event);
                }
                events.push(HumanEvent::Key {
                    key: input.key,
                    modifiers: input.modifiers,
                    t_ms,
                });
            }
        }
        events
    }

    pub fn flush(&mut self) -> Option<HumanEvent> {
        let t_ms = self.run_start_ms.take()?;
        if std::mem::take(&mut self.char_count) == 0 {
            return None;
        }
        Some(HumanEvent::Text { t_ms })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(c: char) -> KeyInput {
        KeyInput {
            text_char: Some(c),
            key: c.to_string(),
            modifiers: Modifiers::default(),
        }
    }

    fn named(key: &str) -> KeyInput {
        KeyInput {
            text_char: None,
            key: key.into(),
            modifiers: Modifiers::default(),
        }
    }

    #[test]
    fn plain_typing_becomes_one_content_free_event() {
        let mut coalescer = Coalescer::new();
        for (index, c) in "secret".chars().enumerate() {
            assert!(coalescer.push(text(c), index as u64).is_empty());
        }
        assert_eq!(coalescer.flush(), Some(HumanEvent::Text { t_ms: 0 }));
    }

    #[test]
    fn enter_flushes_text_then_emits_key() {
        let mut coalescer = Coalescer::new();
        coalescer.push(text('a'), 10);
        let events = coalescer.push(named("Enter"), 20);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], HumanEvent::Text { t_ms: 10 });
        assert!(matches!(
            &events[1],
            HumanEvent::Key { key, .. } if key == "Enter"
        ));
    }

    #[test]
    fn command_keeps_structured_modifiers() {
        let mut coalescer = Coalescer::new();
        let events = coalescer.push(
            KeyInput {
                text_char: Some('c'),
                key: "c".into(),
                modifiers: Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            },
            0,
        );
        assert!(matches!(
            &events[0],
            HumanEvent::Key { key, modifiers, .. }
                if key == "c" && modifiers.ctrl
        ));
    }

    #[test]
    fn backspace_edits_a_pending_run_without_retaining_text() {
        let mut coalescer = Coalescer::new();
        coalescer.push(text('a'), 0);
        coalescer.push(named("Backspace"), 1);
        assert!(coalescer.flush().is_none());
    }

    #[test]
    fn idle_gap_flushes_before_the_next_key() {
        let mut coalescer = Coalescer::new().with_idle_flush(1000);
        coalescer.push(text('a'), 0);
        let events = coalescer.push(text('b'), 1200);
        assert_eq!(events, vec![HumanEvent::Text { t_ms: 0 }]);
        assert_eq!(coalescer.flush(), Some(HumanEvent::Text { t_ms: 1200 }));
    }

    #[test]
    fn backspace_without_a_run_is_a_key() {
        let mut coalescer = Coalescer::new();
        let events = coalescer.push(named("Backspace"), 0);
        assert!(matches!(
            &events[0],
            HumanEvent::Key { key, .. } if key == "Backspace"
        ));
    }
}
