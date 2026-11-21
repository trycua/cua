// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! trycua/cua#7371: background `invoke_menu` on a GTK 3 window opened the
//! menu, whose keyboard grab ate the keystrokes the user was typing into
//! their own window (and a swallowed space ran the item a second time). The
//! user's text must arrive whole, and the item must run once, or the call
//! refuses with nothing run:
//!
//! `DISPLAY=:0 cargo test -p platform-linux --test background_menu_xorg -- --ignored --nocapture`

#![cfg(target_os = "linux")]

mod common;

use std::time::Duration;

use common::*;
use serde_json::json;

#[test]
#[ignore = "needs an X11 session with a window manager, AT-SPI, xdotool, PyQt and GTK 3"]
fn background_invoke_menu_never_eats_the_users_keystrokes() {
    let dir = tempfile::tempdir().unwrap();
    let menu = Fixture::start(GTK_FIXTURE, &["menu"], dir.path(), "cua-honesty-gtk-menu");
    let user = Fixture::start(GTK_FIXTURE, &["user"], dir.path(), "cua-honesty-user-menu");
    let registry = platform_linux::tools::build_registry(false);
    let rt = runtime();
    for _ in 0..3 {
        user.activate();
        xdotool(&["key", "ctrl+a", "BackSpace"]);
        std::thread::sleep(Duration::from_millis(300));
        let before = menu.read()["mark"].as_i64().unwrap();
        let typist = std::thread::spawn(|| {
            xdotool(&["type", "--delay", "80", SENTENCE]);
        });
        std::thread::sleep(Duration::from_millis(1000));
        let result = rt.block_on(registry.invoke(
            "invoke_menu",
            json!({"pid": menu.pid(), "window_id": menu.window, "path": ["File", "Mark"]}),
        ));
        typist.join().unwrap();
        let typed = user.settled()["text"].as_str().unwrap().to_owned();
        let after = menu.read()["mark"].as_i64().unwrap();
        eprintln!(
            "invoke_menu: {} mark {before} -> {after}, user got {typed:?}",
            summary(&result)
        );
        assert_eq!(typed, SENTENCE, "the user's keystrokes were lost");
        if result.is_error == Some(true) {
            assert!(is_refusal(&result), "unexpected error {}", summary(&result));
            assert_eq!(after, before, "the menu item ran under a refusal");
        } else {
            assert_eq!(
                after,
                before + 1,
                "invoke_menu ran the item {} times",
                after - before
            );
        }
    }
}
