// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! trycua/cua#7327: background `type_text` into a Qt `QLineEdit` with a
//! `QIntValidator` put letters in the field and answered "confirmed". Qt
//! applies an accessibility write to a single-line field as `setText`, which
//! skips the validator. The background call types keys instead (the agent's
//! own keyboard on a real Xorg with a writable `/dev/uinput`, `XSendEvent` on
//! Xvfb), which the validator filters, reports what the field holds, and
//! leaves the user's window active and its text alone. With the field hidden,
//! or a menu, combo box list or modal dialog open, it refuses before any key.
//! The foreground call types real keys. A multi-line field still takes the
//! background write. Runs on a real Xorg and on Xvfb + a window manager:
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
plain = QtWidgets.QLineEdit(); plain.setAccessibleName("plain-field")
notes = QtWidgets.QPlainTextEdit(); notes.setAccessibleName("notes-field")
combo = QtWidgets.QComboBox(); combo.addItems(["one", "two"]); combo.setAccessibleName("combo")
for widget in (other, number, spin, plain, notes, combo):
    layout.addWidget(widget)
S = {"menu_fired": 0}
menu = QtWidgets.QMenu(win)
menu.addAction("Mark").triggered.connect(lambda *_: S.__setitem__("menu_fired", S["menu_fired"] + 1))
dialog = QtWidgets.QDialog(win); dialog.setModal(True)
dialog_field = QtWidgets.QLineEdit(dialog)
def command(name):
    if name == "hide": number.hide()
    if name == "menu": menu.popup(win.mapToGlobal(QtCore.QPoint(20, 20)))
    if name == "combo": combo.showPopup()
    if name == "dialog": dialog.show()
    if name == "close": number.show(); menu.hide(); combo.hidePopup(); dialog.hide()
def dump():
    if os.path.exists(OUT + ".reset"):
        number.clear(); spin.setValue(0); plain.clear(); os.remove(OUT + ".reset")
    if os.path.exists(OUT + ".cmd"):
        command(open(OUT + ".cmd").read().strip()); os.remove(OUT + ".cmd")
    S.update({"other": other.text(), "number": number.text(), "spin": spin.text(),
              "plain": plain.text(), "notes": notes.toPlainText(), "active": win.isActiveWindow(),
              "combo": combo.currentText(), "dialog_field": dialog_field.text(),
              "popup": QtWidgets.QApplication.activePopupWidget() is not None,
              "modal": QtWidgets.QApplication.activeModalWidget() is not None})
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

fn reset(qt: &Fixture) -> Value {
    std::fs::write(qt.state.with_extension("json.reset"), "").unwrap();
    qt.settled()
}

fn command(qt: &Fixture, name: &str) -> Value {
    std::fs::write(qt.state.with_extension("json.cmd"), name).unwrap();
    qt.settled()
}

fn active_window() -> u64 {
    xdotool(&["getactivewindow"]).trim().parse().unwrap_or(0)
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
    let agent_keyboard = platform_linux::input::real_keyboard_input_available();
    let route = if agent_keyboard {
        "global_input"
    } else {
        "synthetic_events"
    };
    user.activate();

    // (field, typed, what the field holds after real keys, effect)
    let cases = [
        ("int-field", "number", "12", "12", "confirmed"),
        ("int-field", "number", "AB", "", "suspected_noop"),
        ("int-field", "number", "1a2", "12", "unverifiable"),
        ("spin-field", "spin", "12", "12", "confirmed"),
        ("spin-field", "spin", "AB", "0", "suspected_noop"),
        ("spin-field", "spin", "1a2", "12", "unverifiable"),
        (
            "plain-field",
            "plain",
            "Hi, Zoë {x}!",
            "Hi, Zoë {x}!",
            "confirmed",
        ),
        ("plain-field", "plain", "中文 ok", "中文 ok", "confirmed"),
    ];
    for (label, field, text, held, effect) in cases {
        reset(&qt);
        let user_text = user.read()["text"].clone();
        let field_token = token(&registry, &qt, label);
        let result = type_text(&registry, &qt, &field_token, text, "background");
        let after = qt.settled();
        eprintln!(
            "background {text:?} into {label} (agent keyboard: {agent_keyboard}): {} state {after}",
            summary(&result)
        );
        assert_eq!(after["other"], "", "the focused field took {text:?}");
        assert_eq!(
            active_window(),
            user.window,
            "the user's window lost activation"
        );
        assert_eq!(
            user.read()["text"],
            user_text,
            "the user's window took keys"
        );
        assert_ne!(result.is_error, Some(true), "{}", summary(&result));
        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["effect"], effect, "{}", summary(&result));
        assert_eq!(structured["route"], route, "{}", summary(&result));
        let shown = after[field].as_str().unwrap();
        if field == "spin" && held != "0" {
            assert!(
                shown.contains(held) && shown.chars().all(|c| c.is_ascii_digit()),
                "{label} holds {shown:?} after {text:?}"
            );
        } else {
            assert_eq!(shown, held, "{label} after {text:?}");
        }
    }

    let notes = token(&registry, &qt, "notes-field");
    let result = type_text(&registry, &qt, &notes, "AB", "background");
    let after = qt.settled();
    eprintln!("background notes: {} state {after}", summary(&result));
    assert_ne!(result.is_error, Some(true), "{}", summary(&result));
    assert_eq!(after["notes"], "AB");

    for (text, expected) in [("AB", ""), ("12", "12")] {
        reset(&qt);
        let number = token(&registry, &qt, "int-field");
        let result = type_text(&registry, &qt, &number, text, "foreground");
        let after = qt.settled();
        eprintln!("foreground {text:?}: {} state {after}", summary(&result));
        assert_ne!(result.is_error, Some(true), "{:?}", result.content);
        assert_eq!(after["number"], expected, "foreground {text:?}");
        assert_eq!(after["other"], "", "the other field took {text:?}");
    }

    // A field that takes no keys, or an app whose popup or modal dialog would
    // take them: refused before any key, every widget as it was.
    let number = token(&registry, &qt, "int-field");
    for (blocker, why) in [
        ("hide", "hidden, disabled or read-only"),
        ("menu", "popup, menu or modal dialog"),
        ("combo", "popup, menu or modal dialog"),
        ("dialog", "popup, menu or modal dialog"),
    ] {
        reset(&qt);
        let before = command(&qt, blocker);
        user.activate();
        let user_text = user.read()["text"].clone();
        let result = type_text(&registry, &qt, &number, "12", "background");
        let after = qt.settled();
        command(&qt, "close");
        eprintln!(
            "background \"12\" with {blocker}: {} state {after}",
            summary(&result)
        );
        assert!(is_refusal(&result), "{blocker}: {}", summary(&result));
        let detail = result.structured_content.as_ref().unwrap()["detail"].to_string();
        assert!(detail.contains(why), "{blocker}: {detail}");
        for key in [
            "number",
            "other",
            "spin",
            "plain",
            "combo",
            "dialog_field",
            "menu_fired",
        ] {
            assert_eq!(after[key], before[key], "{blocker} changed {key}");
        }
        assert_eq!(
            user.read()["text"],
            user_text,
            "{blocker}: the user's window took keys"
        );
    }
}
