//! Real native::set_value through private D-Bus, with independent pipe readback.
#![cfg(target_os = "linux")]
use platform_linux::atspi::native;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Fixture {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Fixture {
    fn new() -> Self {
        assert_eq!(std::env::var("CUA_NATIVE_GTK_TEST").as_deref(), Ok("1"));
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
        serde_json::from_str(&line).expect("independent service readback")
    }
    fn request(&mut self, value: Value) -> Value {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
        self.read()
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
#[test]
#[ignore = "requires private dbus-run-session and Python dbus/GI"]
fn unicode_set_value_fallback_preserves_exact_text() {
    let mut f = Fixture::new();
    let text = "First draft — café, mañana.";
    f.set(text).expect("actual native set_value");
    let actual = f.request(json!({"op":"read"}));
    eprintln!("FALLBACK_READBACK {actual}");
    assert_eq!(actual["sets"], 1);
    assert_eq!(actual["inserts"].as_array().unwrap().len(), 1);
    assert_eq!(actual["text"], text, "independent service text");
}

#[test]
#[ignore = "requires private dbus-run-session and Python dbus/GI"]
fn fallback_byte_lengths_keep_character_offsets_and_existing_errors() {
    let mut f = Fixture::new();
    for (initial, caret, text, expected) in [
        ("", 0, "ASCII unchanged", "ASCII unchanged"),
        ("", 0, "🦀🙂", "🦀🙂"),
        ("", 0, "e\u{301}", "e\u{301}"),
        ("café!", 4, "", "café!"),
        ("é🦀Z", 2, "— café", "é🦀— caféZ"),
        ("é🦀Z", 1, "X", "éX🦀Z"),
    ] {
        f.request(json!({"op":"reset", "text":initial, "caret":caret}));
        f.set(text).unwrap();
        let actual = f.request(json!({"op":"read"}));
        eprintln!("FALLBACK_MATRIX {actual}");
        assert_eq!(actual["text"], expected);
        assert_eq!(actual["caret"], caret + text.chars().count());
        assert_eq!(actual["inserts"], json!([[caret, text, text.len()]]));
    }
    for mode in [
        "set-error",
        "caret-error",
        "accept",
        "insert-error",
        "insert-false",
    ] {
        f.request(json!({"op":"reset", "mode":mode}));
        let result = f.set("café");
        let actual = f.request(json!({"op":"read"}));
        eprintln!("FALLBACK_ERROR_CASE mode={mode} result={result:?} state={actual}");
        if mode.starts_with("insert-") {
            assert_eq!(
                result.unwrap_err().to_string(),
                "no_value_route: element 0 exposes neither EditableText nor Value"
            );
            assert_eq!(actual["text"], "");
        } else {
            result.unwrap();
            assert_eq!(actual["text"], "café");
        }
        assert_eq!(actual["sets"], 1);
        assert_eq!(
            actual["inserts"].as_array().unwrap().len(),
            if mode == "accept" { 0 } else { 1 }
        );
    }
}

#[test]
#[ignore = "requires private dbus-run-session and Python dbus/GI"]
fn primary_insert_byte_lengths_preserve_text_on_private_service() {
    let mut f = Fixture::new();
    for (initial, caret, text, expected) in [
        (
            "",
            0,
            "First draft — café, mañana.",
            "First draft — café, mañana.",
        ),
        ("", 0, "ASCII", "ASCII"),
        ("", 0, "🦀🙂", "🦀🙂"),
        ("", 0, "e\u{301}", "e\u{301}"),
        ("café!", 4, "", "café!"),
        ("é🦀Z", 2, "— café", "é🦀— caféZ"),
    ] {
        f.request(json!({"op":"reset", "text":initial, "caret":caret}));
        assert!(native::insert_text(f.child.id(), text).unwrap());
        let actual = f.request(json!({"op":"read"}));
        eprintln!("PRIMARY_SERVICE_READBACK {actual}");
        assert_eq!(actual["text"], expected);
        assert_eq!(actual["caret"], caret + text.chars().count());
        assert_eq!(actual["sets"], 0);
        assert_eq!(actual["inserts"], json!([[caret, text, text.len()]]));
    }
}
