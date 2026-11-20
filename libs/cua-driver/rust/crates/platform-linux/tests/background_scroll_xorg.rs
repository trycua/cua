// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! trycua/cua#7369: background `scroll` on a Qt list answered "Scrolled" and
//! the list did not move. It must move by every detent sent, or refuse.
//! Qt is checked active and inactive. Runs on a real Xorg with a writable
//! `/dev/uinput` (the agent's own pointer: the list must move 3 detents) and
//! on Xvfb + a window manager (no agent pointer: the call must refuse):
//!
//! `DISPLAY=:0 cargo test -p platform-linux --test background_scroll_xorg -- --ignored --nocapture`

#![cfg(target_os = "linux")]

mod common;

use std::time::Duration;

use common::*;
use serde_json::json;

#[test]
#[ignore = "needs an X11 session with a window manager, AT-SPI, xdotool, PyQt and GTK 3"]
fn background_scroll_on_a_qt_list_moves_it_or_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let qt = Fixture::start(
        QT_FIXTURE,
        &[&qt_version()],
        dir.path(),
        "cua-honesty-qt-scroll",
    );
    let user = Fixture::start(
        GTK_FIXTURE,
        &["user"],
        dir.path(),
        "cua-honesty-user-scroll",
    );
    let registry = platform_linux::tools::build_registry(false);
    let rt = runtime();
    for _ in 0..3 {
        for qt_active in [true, false] {
            if qt_active {
                qt.activate();
            } else {
                user.activate();
            }
            let before = qt.read();
            let (x, y) = (before["list"][0].clone(), before["list"][1].clone());
            let result = rt.block_on(registry.invoke_with_native_window_pixels(
                "scroll",
                json!({"pid": qt.pid(), "window_id": qt.window, "x": x, "y": y,
                       "direction": "down", "amount": 3}),
            ));
            let after = qt.settled();
            let moved = after["list_top"].as_i64().unwrap() - before["list_top"].as_i64().unwrap();
            eprintln!(
                "scroll (qt active: {qt_active}): {} list_top {} -> {}",
                summary(&result),
                before["list_top"],
                after["list_top"]
            );
            if result.is_error == Some(true) {
                assert!(is_refusal(&result), "unexpected error {}", summary(&result));
                assert_eq!(moved, 0, "the list moved under a refusal");
            } else {
                // Three detents of three rows each: a lost or reversed detent
                // (Qt re-reading its valuators mid-scroll) shows up here.
                assert_eq!(moved, 9, "scroll reported success and moved {moved} rows");
            }
            xdotool(&[
                "mousemove",
                "--window",
                &qt.window.to_string(),
                &x.to_string(),
                &y.to_string(),
                "click",
                "--repeat",
                "30",
                "4",
            ]);
            std::thread::sleep(Duration::from_millis(300));
        }
    }
}
