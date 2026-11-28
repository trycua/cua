// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `native::set_value` and `native::insert_text` against a private AT-SPI
//! application whose editable rejects `SetTextContents`, run once per toolkit
//! it can claim. The application reports its text, caret and calls over a pipe,
//! not over AT-SPI. See `tests/README.md` for the command.

#![cfg(target_os = "linux")]

use platform_linux::atspi::native;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// What the application answers for `Application.ToolkitName`, and so the unit
/// its `InsertText` reads `length` in. `None` leaves the property unset.
const TOOLKITS: [Option<&str>; 3] = [Some("GTK"), Some("Qt"), None];

const SENTENCE: &str = "First draft — café, mañana.";

/// `length` the toolkit needs to insert all of `text`.
fn insert_length(toolkit: Option<&str>, text: &str) -> usize {
    match toolkit {
        Some("Qt") => text.encode_utf16().count(),
        _ => text.len(),
    }
}

struct Fixture {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Fixture {
    fn new() -> Self {
        assert_eq!(std::env::var("CUA_PRIVATE_ATSPI_TEST").as_deref(), Ok("1"));
        let mut child = Command::new("/usr/bin/python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/rejecting_editable.py"
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut f = Self {
            child,
            input,
            output,
        };
        let ready = f.read();
        assert_eq!(ready["pid"], f.child.id());
        f
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        serde_json::from_str(&line).expect("fixture readback")
    }

    fn request(&mut self, value: Value) -> Value {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
        self.read()
    }

    fn reset(&mut self, toolkit: Option<&str>, text: &str, caret: usize, mode: &str) {
        self.request(
            json!({"op": "reset", "toolkit": toolkit, "text": text, "caret": caret, "mode": mode}),
        );
    }

    fn state(&mut self) -> Value {
        self.request(json!({"op": "read"}))
    }

    fn set(&self, value: &str) -> anyhow::Result<()> {
        native::set_value(self.child.id(), 0, value)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn no_value_route() -> String {
    format!(
        "{}: element 0 exposes neither EditableText nor Value",
        native::NO_VALUE_ROUTE
    )
}

/// (text before, caret in characters, value written, text after)
const WRITES: [(&str, usize, &str, &str); 7] = [
    ("", 0, SENTENCE, SENTENCE),
    ("", 0, "ASCII unchanged", "ASCII unchanged"),
    ("", 0, "🦀🙂", "🦀🙂"),
    ("", 0, "e\u{301}", "e\u{301}"),
    ("café!", 4, "", "café!"),
    ("é🦀Z", 2, "— café", "é🦀— caféZ"),
    ("é🦀Z", 1, "X", "éX🦀Z"),
];

#[test]
#[ignore = "needs a private dbus-run-session with at-spi2-core and Python dbus/GI"]
fn set_value_falls_back_to_insert_text_in_the_toolkit_unit() {
    let mut f = Fixture::new();
    for toolkit in TOOLKITS {
        for (initial, caret, value, expected) in WRITES {
            f.reset(toolkit, initial, caret, "reject");
            let result = f.set(value);
            let actual = f.state();
            eprintln!("FALLBACK toolkit={toolkit:?} result={result:?} state={actual}");
            assert_eq!(actual["sets"], 1, "{toolkit:?} {value:?}");
            if toolkit.is_none() && !value.is_ascii() {
                // No length is safe for an unknown toolkit: nothing is inserted.
                assert_eq!(result.unwrap_err().to_string(), no_value_route());
                assert_eq!(actual["inserts"], json!([]));
                assert_eq!(actual["text"], initial);
                continue;
            }
            result.unwrap_or_else(|e| panic!("{toolkit:?} {value:?}: {e:#}"));
            assert_eq!(actual["text"], expected, "{toolkit:?}");
            assert_eq!(actual["caret"], caret + value.chars().count());
            assert_eq!(
                actual["inserts"],
                json!([[caret, value, insert_length(toolkit, value)]])
            );
        }
    }
}

#[test]
#[ignore = "needs a private dbus-run-session with at-spi2-core and Python dbus/GI"]
fn set_value_fallback_reports_each_editable_answer() {
    let mut f = Fixture::new();
    for toolkit in TOOLKITS {
        for mode in [
            "accept",
            "set-error",
            "caret-error",
            "insert-error",
            "insert-false",
        ] {
            f.reset(toolkit, "", 0, mode);
            let result = f.set("café");
            let actual = f.state();
            eprintln!(
                "FALLBACK_ANSWER toolkit={toolkit:?} mode={mode} result={result:?} state={actual}"
            );
            assert_eq!(actual["sets"], 1, "{toolkit:?} {mode}");
            let inserted = match (mode, toolkit) {
                ("accept", _) => {
                    result.unwrap();
                    assert_eq!(actual["text"], "café");
                    0
                }
                (_, None) => {
                    assert_eq!(result.unwrap_err().to_string(), no_value_route());
                    assert_eq!(actual["text"], "");
                    0
                }
                ("insert-error" | "insert-false", _) => {
                    assert_eq!(result.unwrap_err().to_string(), no_value_route());
                    assert_eq!(actual["text"], "");
                    1
                }
                _ => {
                    result.unwrap();
                    assert_eq!(actual["text"], "café");
                    1
                }
            };
            assert_eq!(
                actual["inserts"].as_array().unwrap().len(),
                inserted,
                "{toolkit:?} {mode}"
            );
        }
    }
}

#[test]
#[ignore = "needs a private dbus-run-session with at-spi2-core and Python dbus/GI"]
fn insert_text_writes_in_the_toolkit_unit() {
    let mut f = Fixture::new();
    for toolkit in TOOLKITS {
        for (initial, caret, text, expected) in WRITES {
            f.reset(toolkit, initial, caret, "reject");
            let written = native::insert_text(f.child.id(), text).unwrap();
            let actual = f.state();
            eprintln!("INSERT toolkit={toolkit:?} written={written} state={actual}");
            assert_eq!(actual["sets"], 0);
            if toolkit.is_none() && !text.is_ascii() {
                assert!(!written, "{text:?}");
                assert_eq!(actual["inserts"], json!([]));
                assert_eq!(actual["text"], initial);
                continue;
            }
            assert!(written, "{toolkit:?} {text:?}");
            assert_eq!(actual["text"], expected, "{toolkit:?}");
            assert_eq!(actual["caret"], caret + text.chars().count());
            assert_eq!(
                actual["inserts"],
                json!([[caret, text, insert_length(toolkit, text)]])
            );
        }
    }
}

#[test]
#[ignore = "needs a private dbus-run-session with at-spi2-core and Python dbus/GI"]
fn no_write_reaches_a_qt_single_line_field() {
    let mut f = Fixture::new();
    for toolkit in TOOLKITS {
        for multiline in [false, true] {
            let reset = json!({"op": "reset", "toolkit": toolkit, "text": "", "caret": 0,
                               "mode": "reject", "multiline": multiline});
            f.request(reset);
            let inserted = native::insert_text(f.child.id(), "AB").map(|written| assert!(written));
            let indexed = native::type_into_editable_at(f.child.id(), 0, "CD");
            let set = f.set("EF");
            let actual = f.state();
            eprintln!(
                "WRITE toolkit={toolkit:?} multiline={multiline} insert={inserted:?} \
                 indexed={indexed:?} set={set:?} state={actual}"
            );
            if toolkit == Some("Qt") && !multiline {
                for result in [inserted, indexed, set] {
                    assert!(result.unwrap_err().is::<native::QtLineEditSetsText>());
                }
                assert_eq!(actual["inserts"], json!([]));
                assert_eq!(actual["sets"], 0);
                assert_eq!(actual["text"], "");
            } else {
                inserted.unwrap();
                indexed.unwrap();
                set.unwrap();
                assert_eq!(actual["sets"], 1, "{toolkit:?} multiline={multiline}");
                assert_eq!(
                    actual["text"], "ABCDEF",
                    "{toolkit:?} multiline={multiline}"
                );
            }
        }
    }
}
