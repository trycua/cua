// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Fixtures for the real-X11 background-delivery checks: a Qt window with a
//! `Ctrl+M` action and a long list, and GTK 3 windows (one with a File >
//! Mark menu, one the "user" types into). Each fixture writes its own state
//! to a JSON file, so results are read from the app, never through the
//! driver. Needs an X11 session with a window manager, AT-SPI, `xdotool`, and
//! a `python3` with PyGObject (GTK 3) and PyQt6 (or PyQt5 with
//! `CUA_TEST_QT=5`).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub const SENTENCE: &str = "the quick brown fox jumps over the lazy dog";

pub const QT_FIXTURE: &str = r#"
import json, os, sys
QV, TITLE, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
if QV == "5":
    from PyQt5 import QtCore, QtGui, QtWidgets
    QAction = QtWidgets.QAction
else:
    from PyQt6 import QtCore, QtGui, QtWidgets
    QAction = QtGui.QAction
app = QtWidgets.QApplication(sys.argv)
win = QtWidgets.QMainWindow(); win.setWindowTitle(TITLE); win.resize(420, 360)
S = {"mark": 0}
mark = QAction("Mark", win); mark.setShortcut(QtGui.QKeySequence("Ctrl+M"))
mark.triggered.connect(lambda *_: S.__setitem__("mark", S["mark"] + 1))
win.menuBar().addMenu("File").addAction(mark)
lst = QtWidgets.QListWidget(); lst.addItems([f"row {i}" for i in range(40)]); lst.setFixedHeight(160)
win.setCentralWidget(lst)
def dump():
    p = lst.mapTo(win, lst.rect().center())
    S.update(list_top=lst.verticalScrollBar().value(), list=[p.x(), p.y()], active=win.isActiveWindow())
    with open(OUT + ".tmp", "w") as f: json.dump(S, f)
    os.replace(OUT + ".tmp", OUT)
t = QtCore.QTimer(); t.timeout.connect(dump); t.start(50)
win.show(); app.exec()
"#;

pub const GTK_FIXTURE: &str = r#"
import json, os, sys, time
import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, GLib
KIND, TITLE, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
S = {"mark": 0, "text": "", "activations": []}
win = Gtk.Window(title=TITLE); win.set_default_size(420, 300); win.connect("destroy", Gtk.main_quit)
box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL); win.add(box)
if KIND == "menu":
    bar = Gtk.MenuBar(); file_item = Gtk.MenuItem(label="File"); menu = Gtk.Menu(); file_item.set_submenu(menu)
    item = Gtk.MenuItem(label="Mark")
    def on_mark(*_):
        S["mark"] += 1; S["activations"].append(time.time())
    item.connect("activate", on_mark)
    menu.append(item); bar.append(file_item); box.pack_start(bar, False, False, 0)
view = Gtk.TextView(); buf = view.get_buffer(); box.pack_start(view, True, True, 0)
def dump():
    S["text"] = buf.get_text(buf.get_start_iter(), buf.get_end_iter(), True); S["active"] = win.is_active()
    with open(OUT + ".tmp", "w") as f: json.dump(S, f)
    os.replace(OUT + ".tmp", OUT); return True
GLib.timeout_add(50, dump)
win.show_all(); view.grab_focus(); Gtk.main()
"#;

pub struct Fixture {
    pub child: Child,
    pub state: PathBuf,
    pub window: u64,
}

impl Fixture {
    pub fn start(code: &str, args: &[&str], dir: &Path, title: &str) -> Fixture {
        let state = dir.join(format!("{title}.json"));
        let mut argv: Vec<String> = vec!["-c".into(), code.into()];
        argv.extend(args.iter().map(|arg| arg.to_string()));
        argv.push(title.into());
        argv.push(state.to_string_lossy().into_owned());
        let child = Command::new("python3")
            .args(&argv)
            .env("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1")
            .env("NO_AT_BRIDGE", "0")
            .spawn()
            .expect("python3");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !state.exists() {
            assert!(Instant::now() < deadline, "{title} never wrote its state");
            std::thread::sleep(Duration::from_millis(100));
        }
        // The toplevel the driver lists, not whatever else carries the title
        // (GTK's client leader, for one).
        let pid = child.id();
        let window = loop {
            if let Some(window) = platform_linux::x11::list_windows(Some(pid))
                .into_iter()
                .find(|window| window.title == title)
            {
                break window.xid;
            }
            assert!(Instant::now() < deadline, "no window titled {title}");
            std::thread::sleep(Duration::from_millis(100));
        };
        // The AT-SPI bridge and the window manager settle after the map.
        std::thread::sleep(Duration::from_millis(1500));
        Fixture {
            child,
            state,
            window,
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn read(&self) -> Value {
        let text = std::fs::read_to_string(&self.state).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    pub fn activate(&self) {
        xdotool(&["windowactivate", &self.window.to_string()]);
        std::thread::sleep(Duration::from_millis(600));
    }

    /// Wait for the fixture's next state dump.
    pub fn settled(&self) -> Value {
        std::thread::sleep(Duration::from_millis(600));
        self.read()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn xdotool(args: &[&str]) -> String {
    let output = Command::new("xdotool")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .expect("xdotool");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn qt_version() -> String {
    std::env::var("CUA_TEST_QT").unwrap_or_else(|_| "6".into())
}

pub fn is_refusal(result: &cua_driver_core::protocol::ToolResult) -> bool {
    result.is_error == Some(true)
        && result
            .structured_content
            .as_ref()
            .is_some_and(|structured| structured["code"] == "background_unavailable")
}

pub fn summary(result: &cua_driver_core::protocol::ToolResult) -> Value {
    json!({ "is_error": result.is_error, "structured": result.structured_content })
}

pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().unwrap()
}
