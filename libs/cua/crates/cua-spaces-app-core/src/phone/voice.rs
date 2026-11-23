// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Push-to-talk dictation into a message box.
//!
//! The phone's own speech recognizer does the listening (on the phone when
//! the language supports it); nothing is sent to a Cua server for
//! transcription. This module owns the words for every state (a permission
//! that was never asked, one that was refused, a recognizer that is not
//! there) and where a transcript lands in the box, so every shell reads and
//! behaves alike.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VoiceInput {
    /// Microphone: `undetermined`, `granted`, `denied`, `restricted`.
    pub microphone: String,
    /// Speech recognition: the same words.
    pub speech: String,
    /// The recognizer can run for the current language.
    pub recognizer_available: bool,
    /// It runs on the phone (no audio leaves it).
    pub on_device: bool,
    pub listening: bool,
    /// What went wrong while listening: `no-speech`, `network`, `audio`, `other`.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub text: String,
    /// `open-settings`, `retry`, or none.
    pub action: Option<String>,
    pub action_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceView {
    /// The mic button is shown at all.
    pub available: bool,
    /// Pressing it starts listening (or asks for permission).
    pub can_start: bool,
    pub button_label: String,
    pub button_hint: String,
    /// "Listening…" while it listens.
    pub status: Option<String>,
    pub notice: Option<Notice>,
    /// Where the audio is handled.
    pub privacy_note: Option<String>,
    /// The next press asks the system for permission.
    pub will_ask: bool,
}

fn denied(text: &str) -> Notice {
    Notice {
        text: text.into(),
        action: Some("open-settings".into()),
        action_label: Some("Open Settings".into()),
    }
}

pub fn view(i: &VoiceInput) -> VoiceView {
    let ask = i.microphone == "undetermined" || i.speech == "undetermined";
    let mut notice = None;
    let mut can_start = true;
    if i.microphone == "denied" {
        notice = Some(denied(
            "Microphone access is off. Turn it on in Settings to dictate.",
        ));
        can_start = false;
    } else if i.speech == "denied" {
        notice = Some(denied(
            "Speech recognition is off for Cua Spaces. Turn it on in Settings to dictate.",
        ));
        can_start = false;
    } else if i.microphone == "restricted" || i.speech == "restricted" {
        notice = Some(Notice {
            text: "Dictation is restricted on this phone.".into(),
            action: None,
            action_label: None,
        });
        can_start = false;
    } else if !i.recognizer_available && !ask {
        notice = Some(Notice {
            text: "Dictation isn't available for this language right now.".into(),
            action: None,
            action_label: None,
        });
        can_start = false;
    } else if let Some(e) = &i.error {
        let text = match e.as_str() {
            "no-speech" => "Didn't hear anything. Try again.",
            "network" => {
                "Dictation needs a connection for this language. Try again when you're online."
            }
            "audio" => "Couldn't use the microphone. Another app may be using it.",
            _ => "Dictation stopped. Try again.",
        };
        notice = Some(Notice {
            text: text.into(),
            action: Some("retry".into()),
            action_label: Some("Try again".into()),
        });
    }
    VoiceView {
        available: true,
        can_start: can_start || i.listening,
        button_label: if i.listening {
            "Stop dictating".into()
        } else {
            "Dictate".into()
        },
        button_hint: if i.listening {
            "Stops listening and keeps what you said.".into()
        } else if ask {
            "Asks for microphone and speech access, then listens.".into()
        } else {
            "Listens and types what you say.".into()
        },
        status: i.listening.then(|| "Listening\u{2026}".to_string()),
        privacy_note: if i.listening || ask {
            Some(if i.on_device {
                "Dictation stays on this phone.".to_string()
            } else {
                "Dictation uses your phone's built-in speech service.".to_string()
            })
        } else {
            None
        },
        will_ask: ask && !i.listening,
        notice,
    }
}

/// UTF-16 offset (what a text box reports) to a byte index of `s`.
fn byte_at(s: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16();
    }
    s.len()
}

/// The text after dictating `transcript` over the selection `[start, end)`
/// (UTF-16 offsets) of `text`, and where the cursor lands (UTF-16). A space is
/// added where the transcript would touch a word.
pub fn insert(text: &str, start: usize, end: usize, transcript: &str) -> (String, usize) {
    let t = transcript.trim();
    if t.is_empty() {
        let at = start.min(text.encode_utf16().count());
        return (text.to_string(), at);
    }
    let (a, b) = (start.min(end), start.max(end));
    let (ai, bi) = (byte_at(text, a), byte_at(text, b));
    let (before, after) = (&text[..ai], &text[bi..]);
    let lead = if before.is_empty() || before.ends_with(char::is_whitespace) {
        ""
    } else {
        " "
    };
    let trail = if after.is_empty()
        || after.starts_with(char::is_whitespace)
        || after.starts_with(['.', ',', '!', '?', ';', ':'])
    {
        ""
    } else {
        " "
    };
    let inserted = format!("{lead}{t}{trail}");
    let out = format!("{before}{inserted}{after}");
    let cursor =
        before.encode_utf16().count() + format!("{lead}{t}").encode_utf16().count() + trail.len();
    (out, cursor)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Inserted {
    pub text: String,
    pub cursor: usize,
}

pub fn insert_view(text: &str, start: usize, end: usize, transcript: &str) -> Inserted {
    let (text, cursor) = insert(text, start, end, transcript);
    Inserted { text, cursor }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(mic: &str, speech: &str) -> VoiceInput {
        VoiceInput {
            microphone: mic.into(),
            speech: speech.into(),
            recognizer_available: true,
            on_device: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_first_press_asks_and_says_so() {
        let v = view(&input("undetermined", "undetermined"));
        assert!(v.will_ask && v.can_start && v.notice.is_none());
        assert_eq!(v.button_label, "Dictate");
        assert_eq!(
            v.button_hint,
            "Asks for microphone and speech access, then listens."
        );
        assert_eq!(
            v.privacy_note.as_deref(),
            Some("Dictation stays on this phone.")
        );
    }

    #[test]
    fn a_refused_permission_says_where_to_turn_it_on() {
        let v = view(&input("denied", "granted"));
        assert!(!v.can_start);
        let n = v.notice.unwrap();
        assert_eq!(
            n.text,
            "Microphone access is off. Turn it on in Settings to dictate."
        );
        assert_eq!(n.action.as_deref(), Some("open-settings"));
        let s = view(&input("granted", "denied")).notice.unwrap();
        assert!(s.text.starts_with("Speech recognition is off"));
        let r = view(&input("restricted", "granted"));
        assert!(!r.can_start && r.notice.unwrap().action.is_none());
    }

    #[test]
    fn no_recognizer_for_the_language_is_said_plainly() {
        let mut i = input("granted", "granted");
        i.recognizer_available = false;
        let v = view(&i);
        assert!(!v.can_start);
        assert_eq!(
            v.notice.unwrap().text,
            "Dictation isn't available for this language right now."
        );
    }

    #[test]
    fn listening_shows_status_stop_and_where_audio_goes() {
        let mut i = input("granted", "granted");
        i.listening = true;
        let v = view(&i);
        assert_eq!(v.status.as_deref(), Some("Listening\u{2026}"));
        assert_eq!(v.button_label, "Stop dictating");
        assert!(v.can_start, "the button stops it");
        i.on_device = false;
        assert_eq!(
            view(&i).privacy_note.as_deref(),
            Some("Dictation uses your phone's built-in speech service.")
        );
    }

    #[test]
    fn errors_offer_try_again() {
        let mut i = input("granted", "granted");
        i.error = Some("no-speech".into());
        let n = view(&i).notice.unwrap();
        assert_eq!(n.text, "Didn't hear anything. Try again.");
        assert_eq!(n.action.as_deref(), Some("retry"));
    }

    #[test]
    fn a_transcript_lands_at_the_cursor_with_a_space_where_it_touches_a_word() {
        assert_eq!(
            insert("", 0, 0, "fix the test"),
            ("fix the test".into(), 12)
        );
        assert_eq!(
            insert("please", 6, 6, "fix it"),
            ("please fix it".into(), 13)
        );
        assert_eq!(insert("a b", 1, 1, "x"), ("a x b".into(), 3));
        assert_eq!(
            insert("hello world", 6, 11, "there"),
            ("hello there".into(), 11)
        );
        assert_eq!(insert("keep", 4, 4, "  "), ("keep".into(), 4));
        // Punctuation after the cursor needs no space before it.
        assert_eq!(insert("done.", 4, 4, "now"), ("done now.".into(), 8));
        // A selection given backwards works; offsets past the end clamp.
        assert_eq!(insert("ab", 9, 9, "c"), ("ab c".into(), 4));
        assert_eq!(insert("hello world", 11, 6, "there").0, "hello there");
    }

    #[test]
    fn offsets_are_utf16_units_not_characters() {
        // The emoji is two UTF-16 units; the cursor after it is 2, then the transcript follows.
        let (t, c) = insert("\u{1F600}", 2, 2, "ok");
        assert_eq!(t, "\u{1F600} ok");
        assert_eq!(c, 5);
    }
}
