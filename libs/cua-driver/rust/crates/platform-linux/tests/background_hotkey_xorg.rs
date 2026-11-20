// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! trycua/cua#7370: `hotkey` to an inactive Qt window answered "Pressed" and
//! the shortcut never fired. It must fire exactly once, or refuse. Runs on a
//! real Xorg with a writable `/dev/uinput` (the agent's own keyboard: Qt runs
//! the shortcut) and on Xvfb + a window manager (synthetic keys only: the
//! chord to the inactive window must refuse, the active one still fires):
//!
//! `DISPLAY=:0 cargo test -p platform-linux --test background_hotkey_xorg -- --ignored --nocapture`

#![cfg(target_os = "linux")]

mod common;

use common::*;
use serde_json::json;

#[test]
#[ignore = "needs an X11 session with a window manager, AT-SPI, xdotool, PyQt and GTK 3"]
fn background_hotkey_to_an_inactive_qt_window_fires_or_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let qt = Fixture::start(
        QT_FIXTURE,
        &[&qt_version()],
        dir.path(),
        "cua-honesty-qt-hotkey",
    );
    let user = Fixture::start(
        GTK_FIXTURE,
        &["user"],
        dir.path(),
        "cua-honesty-user-hotkey",
    );
    let registry = platform_linux::tools::build_registry(false);
    let rt = runtime();
    for _ in 0..3 {
        for qt_active in [false, true] {
            if qt_active {
                qt.activate();
            } else {
                user.activate();
            }
            let before = qt.read()["mark"].as_i64().unwrap();
            let result = rt.block_on(registry.invoke(
                "hotkey",
                json!({"pid": qt.pid(), "window_id": qt.window, "keys": ["ctrl", "m"]}),
            ));
            let after = qt.settled()["mark"].as_i64().unwrap();
            eprintln!(
                "hotkey (qt active: {qt_active}): {} mark {before} -> {after}",
                summary(&result)
            );
            if result.is_error == Some(true) {
                assert!(is_refusal(&result), "unexpected error {}", summary(&result));
                assert!(!qt_active, "a hotkey to the active Qt window was refused");
                assert_eq!(after, before, "the shortcut fired under a refusal");
            } else {
                assert_eq!(
                    after,
                    before + 1,
                    "hotkey reported success and fired {} times",
                    after - before
                );
            }
        }
    }
}
