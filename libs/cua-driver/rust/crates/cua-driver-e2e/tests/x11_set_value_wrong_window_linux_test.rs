// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Linux X11 `set_value` foreground keyboard fallback (`path=click_type_fg`
//! and sibling `path=click_select_fg`) must refuse with
//! `element_bounds_unavailable` and send no pointer or keyboard input when the
//! cached element is not showing, lies outside the target window, is owned by
//! another control, or is covered by another window (#7945).
//!
//! Spawns two real GTK3 X11 windows in separate processes:
//! - `Cua7945Bystander` (`Gtk.TextView`) sitting behind `Cua7945TargetForm`,
//!   and optionally raised `keep_above` to cover the target field's point;
//! - `Cua7945TargetForm` containing a `Gtk.ScrolledWindow` with a field
//!   (`BlockerField`) exposing AT-SPI `Text` + `Component` and no
//!   `EditableText`/`Value` (so `set_value` takes `click_type_fg`), a combo
//!   control (`BlockerCombo`) with `Atk.Role.COMBO_BOX` and no accessibility
//!   write route (so `set_value` takes `click_select_fg`), plus a sibling
//!   `OtherInWindowField` (`Gtk.Entry`).
//!
//! #[ignore] (needs an X11 display with a window manager, AT-SPI, and GTK3). Run:
//!   cargo test -p cua-driver-e2e --test x11_set_value_wrong_window_linux_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::{ax, spawn_in_job, Driver, McpDriver, ToolResponse};
use serde_json::{json, Value};
use tempfile::TempDir;

const BYSTANDER_PY: &str = r#"
import json, os, sys
import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, GLib

state_path, ctrl_path = sys.argv[1], sys.argv[2]
win = Gtk.Window(title="Cua7945Bystander")
win.set_default_size(900, 650)
win.move(40, 40)
tv = Gtk.TextView()
tv.set_wrap_mode(Gtk.WrapMode.CHAR)
buf = tv.get_buffer()
clicks = [0]

def on_button_press(_widget, _event):
    clicks[0] += 1
    return False

tv.connect("button-press-event", on_button_press)
win.add(tv)
win.show_all()

def poll():
    if os.path.exists(ctrl_path):
        with open(ctrl_path) as f:
            cmd = f.read().strip()
        os.unlink(ctrl_path)
        if cmd == "clear":
            buf.set_text("")
            clicks[0] = 0
        elif cmd == "keep_above_on":
            win.set_keep_above(True)
            win.present()
        elif cmd == "keep_above_off":
            win.set_keep_above(False)
    start, end = buf.get_bounds()
    text = buf.get_text(start, end, True)
    with open(state_path + ".tmp", "w") as f:
        json.dump({"pid": os.getpid(), "text": text, "clicks": clicks[0]}, f)
    os.replace(state_path + ".tmp", state_path)
    return True

GLib.timeout_add(60, poll)
GLib.timeout_add_seconds(120, Gtk.main_quit)
Gtk.main()
"#;

const TARGET_PY: &str = r#"
import json, os, sys
import gi
gi.require_version("Gtk", "3.0")
gi.require_version("Atk", "1.0")
from gi.repository import Gtk, Gdk, GLib, Atk

state_path, ctrl_path = sys.argv[1], sys.argv[2]
win = Gtk.Window(title="Cua7945TargetForm")
win.set_default_size(420, 480)
win.move(100, 60)

sw = Gtk.ScrolledWindow()
sw.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.ALWAYS)
box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)

other_entry = Gtk.Entry()
other_entry.set_size_request(340, 34)
other_entry.get_accessible().set_name("OtherInWindowField")

spacer = Gtk.DrawingArea()
spacer.set_size_request(380, 300)

field = Gtk.Label(label="")
field.set_size_request(340, 36)
field.set_can_focus(True)
field.set_selectable(True)
acc = field.get_accessible()
acc.set_role(Atk.Role.PUSH_BUTTON)
acc.set_name("BlockerField")

combo = Gtk.Button(label="Current")
combo.set_size_request(340, 36)
combo_acc = combo.get_accessible()
combo_acc.set_role(Atk.Role.COMBO_BOX)
combo_acc.set_name("BlockerCombo")

field_text = [""]
selected_all = [False]

def on_key_press(widget, event):
    if event.state & Gdk.ModifierType.CONTROL_MASK and event.keyval in (Gdk.KEY_a, Gdk.KEY_A):
        selected_all[0] = True
        return True
    if event.keyval == Gdk.KEY_Tab:
        return False
    codepoint = Gdk.keyval_to_unicode(event.keyval)
    ch = chr(codepoint) if codepoint else ""
    if ch and ch.isprintable():
        if selected_all[0]:
            field_text[0] = ch
            selected_all[0] = False
        else:
            field_text[0] += ch
        field.set_text(field_text[0])
        return True
    return False

field.connect("key-press-event", on_key_press)

box.pack_start(other_entry, False, False, 0)
box.pack_start(spacer, False, False, 0)
box.pack_start(field, False, False, 0)
box.pack_start(combo, False, False, 0)
sw.add(box)
win.add(sw)
win.show_all()
other_entry.grab_focus()
win.present()

def poll():
    if os.path.exists(ctrl_path):
        with open(ctrl_path) as f:
            cmd = f.read().strip()
        os.unlink(ctrl_path)
        adj = sw.get_vadjustment()
        if cmd == "shrink_scroll_out":
            win.resize(420, 220)
            adj.set_value(0)
            other_entry.grab_focus()
        elif cmd == "move_away":
            win.move(620, 60)
            other_entry.grab_focus()
        elif cmd == "swap_other_over_field":
            box.reorder_child(field, 0)
            box.reorder_child(spacer, 1)
            box.reorder_child(other_entry, 2)
            box.reorder_child(combo, 3)
            other_entry.grab_focus()
        elif cmd == "expand_visible":
            box.reorder_child(other_entry, 0)
            box.reorder_child(spacer, 1)
            box.reorder_child(field, 2)
            box.reorder_child(combo, 3)
            win.move(100, 60)
            win.resize(420, 480)
            adj.set_value(0)
            other_entry.grab_focus()
            win.present()
        elif cmd == "clear":
            field_text[0] = ""
            field.set_text("")
            other_entry.set_text("")
            other_entry.grab_focus()
    with open(state_path + ".tmp", "w") as f:
        json.dump({
            "pid": os.getpid(),
            "text": field_text[0],
            "other_text": other_entry.get_text(),
        }, f)
    os.replace(state_path + ".tmp", state_path)
    return True

GLib.timeout_add(60, poll)
GLib.timeout_add_seconds(120, Gtk.main_quit)
Gtk.main()
"#;

fn native_x11_session() -> bool {
    if std::env::var_os("DISPLAY").is_some() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return true;
    }
    if std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some() {
        panic!("x11_set_value_wrong_window_linux_test requires a native X11 session");
    }
    eprintln!("[x11-set-value-wrong-window] not a native X11 session; skipping");
    false
}

struct PyApp {
    child: Child,
    state_path: PathBuf,
    ctrl_path: PathBuf,
}

impl PyApp {
    fn spawn(dir: &Path, name: &str, code: &str) -> Self {
        let state_path = dir.join(format!("{name}_state.json"));
        let ctrl_path = dir.join(format!("{name}_ctrl.txt"));
        let mut cmd = Command::new("/usr/bin/python3");
        cmd.arg("-c")
            .arg(code)
            .arg(&state_path)
            .arg(&ctrl_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        let child = spawn_in_job(&mut cmd).unwrap_or_else(|e| panic!("spawn {name}: {e}"));
        Self {
            child,
            state_path,
            ctrl_path,
        }
    }

    fn wait_state(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(raw) = fs::read_to_string(&self.state_path) {
                if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                    return v;
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("timed out waiting for {}", self.state_path.display());
    }

    fn send_cmd(&self, cmd: &str) {
        fs::write(&self.ctrl_path, format!("{cmd}\n")).expect("write control command");
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.ctrl_path.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(40));
        }
        assert!(
            !self.ctrl_path.exists(),
            "fixture did not consume command {cmd:?}"
        );
        thread::sleep(Duration::from_millis(250));
    }
}

impl Drop for PyApp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn find_window(driver: &mut McpDriver, title: &str) -> (u32, u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let response = driver.call("list_windows", json!({}));
        if let Some(windows) = response.structured()["windows"].as_array() {
            for w in windows {
                if w["title"].as_str() == Some(title) {
                    let pid = w["pid"].as_u64().unwrap_or(0) as u32;
                    let wid = w["window_id"].as_u64().unwrap_or(0);
                    if pid != 0 && wid != 0 {
                        return (pid, wid);
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(150));
    }
    panic!("window {title:?} never appeared in list_windows");
}

fn snapshot_element_token(
    driver: &mut McpDriver,
    pid: u32,
    wid: u64,
    needle: &str,
    label: &str,
) -> String {
    let state = driver.call(
        "get_window_state",
        json!({ "pid": pid, "window_id": wid, "include_screenshot": false }),
    );
    assert!(
        !state.is_error(),
        "snapshot ({label}) failed: {}",
        state.text()
    );
    let idx = ax::element_index_containing(state.tree_text(), needle).unwrap_or_else(|| {
        panic!(
            "{needle} not found in AT-SPI tree ({label}):\n{}",
            state.tree_text()
        )
    });
    format!("{}:{idx}", state.snapshot_id())
}

fn assert_bounds_unavailable(resp: &ToolResponse, case: &str, expected_reason_substr: &str) {
    let s = resp.structured();
    assert!(
        resp.is_error(),
        "{case}: set_value must fail, got ok: {} / {s}",
        resp.text()
    );
    let code = s
        .pointer("/refusal/code")
        .or_else(|| s.get("code"))
        .and_then(Value::as_str);
    assert_eq!(
        code,
        Some("element_bounds_unavailable"),
        "{case}: expected element_bounds_unavailable, got {s}"
    );
    assert_eq!(
        resp.action_effect(),
        Some("none"),
        "{case}: expected effect none, got {s}"
    );
    let reason = s["reason"].as_str().unwrap_or("");
    assert!(
        reason.contains(expected_reason_substr),
        "{case}: expected reason containing {expected_reason_substr:?}, got {s}"
    );
}

#[test]
#[ignore = "requires an isolated X11 display with a window manager, AT-SPI, and GTK3"]
fn x11_foreground_set_value_refuses_stale_and_occluded_targets() {
    if !native_x11_session() {
        return;
    }
    let dir = TempDir::new().expect("create fixture temp dir");
    let bystander = PyApp::spawn(dir.path(), "bystander", BYSTANDER_PY);
    let _ = bystander.wait_state();
    let target = PyApp::spawn(dir.path(), "target", TARGET_PY);
    let _ = target.wait_state();

    let mut driver = McpDriver::spawn_unrecorded().expect("start source-built Linux driver");
    let _ = find_window(&mut driver, "Cua7945Bystander");
    let (target_pid, target_wid) = find_window(&mut driver, "Cua7945TargetForm");

    // 1. Scrolled-off field with bystander behind the stale point: must refuse
    //    with `element_bounds_unavailable` (effect none) and leak nothing.
    let token1 = snapshot_element_token(
        &mut driver,
        target_pid,
        target_wid,
        "BlockerField",
        "scrolled-off",
    );
    target.send_cmd("shrink_scroll_out");
    let res1 = driver.call(
        "set_value",
        json!({
            "pid": target_pid,
            "element_token": token1,
            "value": "LHP-C17",
            "delivery_mode": "foreground",
        }),
    );
    thread::sleep(Duration::from_millis(200));
    let bs1 = bystander.wait_state();
    let ts1 = target.wait_state();
    assert_bounds_unavailable(&res1, "scrolled-off", "not showing");
    assert_eq!(
        bs1["text"].as_str(),
        Some(""),
        "text leaked into bystander behind scrolled-off field: {bs1}"
    );
    assert_eq!(ts1["text"].as_str(), Some(""));

    // 2. Target window moves away while field stays `showing=True`, leaving the
    //    cached centre outside target bounds over bystander.
    target.send_cmd("expand_visible");
    target.send_cmd("clear");
    bystander.send_cmd("clear");
    let token2 = snapshot_element_token(
        &mut driver,
        target_pid,
        target_wid,
        "BlockerField",
        "moved-away",
    );
    target.send_cmd("move_away");
    let res2 = driver.call(
        "set_value",
        json!({
            "pid": target_pid,
            "element_token": token2,
            "value": "MOVED-AWAY",
            "delivery_mode": "foreground",
        }),
    );
    thread::sleep(Duration::from_millis(200));
    let bs2 = bystander.wait_state();
    assert_bounds_unavailable(&res2, "moved-away", "outside its");
    assert_eq!(
        bs2["text"].as_str(),
        Some(""),
        "text leaked into bystander after target moved away: {bs2}"
    );

    // 3. Another control in the same window takes over the cached centre while
    //    `BlockerField` stays `showing=True` inside the window (`point_ownership`).
    target.send_cmd("expand_visible");
    target.send_cmd("clear");
    bystander.send_cmd("clear");
    let token3 = snapshot_element_token(
        &mut driver,
        target_pid,
        target_wid,
        "BlockerField",
        "same-window-swap",
    );
    target.send_cmd("swap_other_over_field");
    let res3 = driver.call(
        "set_value",
        json!({
            "pid": target_pid,
            "element_token": token3,
            "value": "SWAPPED-OVER",
            "delivery_mode": "foreground",
        }),
    );
    thread::sleep(Duration::from_millis(200));
    let ts3 = target.wait_state();
    assert_bounds_unavailable(&res3, "same-window-swap", "point_owned_by_another_element");
    assert!(
        res3.structured()["owner"]
            .as_str()
            .is_some_and(|owner| !owner.is_empty()),
        "expected point_ownership owner field to be populated, got {}",
        res3.structured()
    );
    assert_eq!(
        ts3["other_text"].as_str(),
        Some(""),
        "keys leaked into sibling occupying cached point: {ts3}"
    );

    // 4. Field visible and inside target bounds, but covered by another app's
    //    always-on-top window at click time (X11 `point_owned_by_window`).
    target.send_cmd("expand_visible");
    target.send_cmd("clear");
    bystander.send_cmd("clear");
    let token4 = snapshot_element_token(
        &mut driver,
        target_pid,
        target_wid,
        "BlockerField",
        "occluded",
    );
    bystander.send_cmd("keep_above_on");
    let res4 = driver.call(
        "set_value",
        json!({
            "pid": target_pid,
            "element_token": token4,
            "value": "OCCLUDED",
            "delivery_mode": "foreground",
        }),
    );
    thread::sleep(Duration::from_millis(200));
    let bs4 = bystander.wait_state();
    assert_bounds_unavailable(&res4, "occluded", "covered by topmost window");
    assert_eq!(
        bs4["text"].as_str(),
        Some(""),
        "text leaked into covering window: {bs4}"
    );
    assert_eq!(
        bs4["clicks"].as_u64(),
        Some(0),
        "click leaked into covering window: {bs4}"
    );

    // 5. Sibling `click_select_fg` route (`pick_from_popup`): combo box with no
    //    accessibility write route covered by the always-on-top bystander must
    //    refuse before any click inside `pick_from_popup`.
    bystander.send_cmd("keep_above_off");
    target.send_cmd("expand_visible");
    target.send_cmd("clear");
    bystander.send_cmd("clear");
    let token5 = snapshot_element_token(
        &mut driver,
        target_pid,
        target_wid,
        "BlockerCombo",
        "occluded-combo",
    );
    bystander.send_cmd("keep_above_on");
    let res5 = driver.call(
        "set_value",
        json!({
            "pid": target_pid,
            "element_token": token5,
            "value": "OptionA",
            "delivery_mode": "foreground",
        }),
    );
    thread::sleep(Duration::from_millis(200));
    let bs5 = bystander.wait_state();
    assert_bounds_unavailable(&res5, "occluded-combo", "covered by topmost window");
    assert_eq!(
        bs5["clicks"].as_u64(),
        Some(0),
        "combo click leaked into covering window: {bs5}"
    );
    assert_eq!(
        bs5["text"].as_str(),
        Some(""),
        "combo text leaked into covering window: {bs5}"
    );
    bystander.send_cmd("keep_above_off");

    // 6. Positive control: visible, uncovered field still receives the value.
    target.send_cmd("expand_visible");
    target.send_cmd("clear");
    bystander.send_cmd("clear");
    let token6 = snapshot_element_token(
        &mut driver,
        target_pid,
        target_wid,
        "BlockerField",
        "visible-positive",
    );
    let res6 = driver.call(
        "set_value",
        json!({
            "pid": target_pid,
            "element_token": token6,
            "value": "LHP-C17",
            "delivery_mode": "foreground",
        }),
    );
    thread::sleep(Duration::from_millis(200));
    let bs6 = bystander.wait_state();
    let ts6 = target.wait_state();
    assert!(
        !res6.is_error(),
        "visible set_value must succeed: {} / {}",
        res6.text(),
        res6.structured()
    );
    assert_eq!(res6.action_effect(), Some("confirmed"));
    assert_eq!(ts6["text"].as_str(), Some("LHP-C17"));
    assert_eq!(bs6["text"].as_str(), Some(""));
}
