// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! trycua/cua#7327: background `type_text` into a Qt `QLineEdit` with a
//! `QIntValidator` put letters in the field and answered "confirmed". Qt
//! applies an accessibility write to a single-line field (a line edit or a
//! spin box) as `setText`, which skips the validator. The background call
//! must leave every field as it was and refuse; the foreground call types
//! real keys, which the validator filters. A multi-line field still takes the
//! background write. Runs on Xvfb + a window manager:
//!
//! `DISPLAY=:0 cargo test -p platform-linux --test type_text_qt_line_edit_xorg -- --ignored --nocapture`

#![cfg(target_os = "linux")]

mod common;

use common::*;
use serde_json::{json, Value};

const QT_FORM_FIXTURE: &str = r#"
import json, os, sys
QV, TITLE, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
if QV == "5":
    from PyQt5 import QtCore, QtGui, QtWidgets
else:
    from PyQt6 import QtCore, QtGui, QtWidgets
app = QtWidgets.QApplication(sys.argv)
win = QtWidgets.QWidget(); win.setWindowTitle(TITLE); win.resize(420, 300)
layout = QtWidgets.QVBoxLayout(win)
other = QtWidgets.QLineEdit(); other.setAccessibleName("other-field")
number = QtWidgets.QLineEdit(); number.setAccessibleName("int-field")
number.setValidator(QtGui.QIntValidator(0, 999999, number))
spin = QtWidgets.QSpinBox(); spin.setAccessibleName("spin-field"); spin.setRange(0, 999999)
notes = QtWidgets.QPlainTextEdit(); notes.setAccessibleName("notes-field")
for widget in (other, number, spin, notes):
    layout.addWidget(widget)
def dump():
    S = {"other": other.text(), "number": number.text(), "spin": spin.text(), "notes": notes.toPlainText(), "active": win.isActiveWindow()}
    with open(OUT + ".tmp", "w") as f: json.dump(S, f)
    os.replace(OUT + ".tmp", OUT)
t = QtCore.QTimer(); t.timeout.connect(dump); t.start(50)
win.show(); other.setFocus(); app.exec()
"#;

fn token(registry: &cua_driver_core::tool::ToolRegistry, qt: &Fixture, label: &str) -> Value {
    let state = runtime().block_on(registry.invoke(
        "get_window_state",
        json!({"pid": qt.pid(), "window_id": qt.window, "tree_format": "elements"}),
    ));
    let structured = state
        .structured_content
        .expect("get_window_state structured");
    structured["elements"]
        .as_array()
        .expect("elements")
        .iter()
        .find(|element| element["label"] == label)
        .unwrap_or_else(|| panic!("no element labelled {label}"))["element_token"]
        .clone()
}

fn type_text(
    registry: &cua_driver_core::tool::ToolRegistry,
    qt: &Fixture,
    token: &Value,
    text: &str,
    mode: &str,
) -> cua_driver_core::protocol::ToolResult {
    runtime().block_on(registry.invoke(
        "type_text",
        json!({"pid": qt.pid(), "window_id": qt.window, "element_token": token,
               "text": text, "delivery_mode": mode}),
    ))
}

#[test]
#[ignore = "needs an X11 session with a window manager, AT-SPI, xdotool, PyQt and GTK 3"]
fn type_text_never_writes_past_a_qt_line_edit_validator() {
    let dir = tempfile::tempdir().unwrap();
    let qt = Fixture::start(
        QT_FORM_FIXTURE,
        &[&qt_version()],
        dir.path(),
        "cua-qt-line-edit-validator",
    );
    let user = Fixture::start(GTK_FIXTURE, &["user"], dir.path(), "cua-qt-line-edit-user");
    let registry = platform_linux::tools::build_registry(false);
    user.activate();

    for (label, field, cleared) in [("int-field", "number", ""), ("spin-field", "spin", "0")] {
        let target = token(&registry, &qt, label);
        for text in ["AB", "12"] {
            let result = type_text(&registry, &qt, &target, text, "background");
            let after = qt.settled();
            eprintln!(
                "background {text:?} into {label}: {} state {after}",
                summary(&result)
            );
            assert!(
                is_refusal(&result),
                "background type_text into {label} answered {}",
                summary(&result)
            );
            let detail = result.structured_content.as_ref().unwrap()["detail"].to_string();
            assert!(detail.contains("setText"), "{label}: {detail}");
            assert_eq!(after[field], cleared, "{label} took {text:?}");
            assert_eq!(after["other"], "", "the focused field took {text:?}");
        }
    }

    let notes = token(&registry, &qt, "notes-field");
    let result = type_text(&registry, &qt, &notes, "AB", "background");
    let after = qt.settled();
    eprintln!("background notes: {} state {after}", summary(&result));
    assert_ne!(result.is_error, Some(true), "{}", summary(&result));
    assert_eq!(after["notes"], "AB");

    let number = token(&registry, &qt, "int-field");
    for (text, expected) in [("AB", ""), ("12", "12")] {
        let result = type_text(&registry, &qt, &number, text, "foreground");
        let after = qt.settled();
        eprintln!("foreground {text:?}: {} state {after}", summary(&result));
        assert_ne!(result.is_error, Some(true), "{}", summary(&result));
        assert_eq!(after["number"], expected, "foreground {text:?}");
        assert_eq!(after["other"], "", "the other field took {text:?}");
    }
}
