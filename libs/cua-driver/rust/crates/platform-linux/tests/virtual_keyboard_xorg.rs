// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The virtual master keyboard (`path=mpx_uinput`) against a real Xorg with
//! libinput hotplug and a writable `/dev/uinput`; Xvfb has neither. The X
//! server must run as the test's user so the test can stop its input thread
//! (ptrace):
//!
//! `CUA_TEST_XORG_PID=<pid> DISPLAY=:5 cargo test -p platform-linux --test virtual_keyboard_xorg -- --ignored --nocapture`

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use platform_linux::input::{forget_master_pointer, send_virtual_keyboard_text_paced, TypePacing};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;

/// Thirteen characters no stock layout has, so each one borrows a spare keycode.
const TEXT: &str = "中文输入法测试汉字键盘映射";

#[test]
#[ignore = "requires a real Xorg (CUA_TEST_XORG_PID) with libinput hotplug and /dev/uinput"]
fn a_stalled_server_never_reads_borrowed_keys_after_their_keycodes_are_restored() {
    let server = std::env::var("CUA_TEST_XORG_PID")
        .expect("CUA_TEST_XORG_PID names the X server whose input thread is stalled");
    let input_thread: libc::pid_t = std::fs::read_dir(format!("/proc/{server}/task"))
        .unwrap()
        .map(|task| task.unwrap().path())
        .find(|task| {
            std::fs::read_to_string(task.join("comm"))
                .is_ok_and(|comm| comm.trim() == "InputThread")
        })
        .expect("Xorg runs with an input thread")
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    let window = conn.generate_id().unwrap();
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        200,
        100,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .override_redirect(1)
            .event_mask(EventMask::KEY_PRESS),
    )
    .unwrap();
    conn.map_window(window).unwrap();
    conn.get_input_focus().unwrap().reply().unwrap();
    let cursor = "virtual-keyboard-xorg-stall";
    // Create the master pair and its keyboard before the timed call.
    send_virtual_keyboard_text_paced(cursor, u64::from(window), "a", TypePacing::default())
        .unwrap();
    while conn.poll_for_event().unwrap().is_some() {}

    // Paced: 40 ms a character, a confirm every 4 characters. Only the
    // thread that reads evdev stops, as under CPU starvation: the server
    // still answers requests, so the keymap restore is not held back.
    let stall = std::thread::spawn(move || unsafe {
        std::thread::sleep(Duration::from_millis(250));
        let null = std::ptr::null_mut::<libc::c_void>();
        assert_eq!(
            libc::ptrace(libc::PTRACE_SEIZE, input_thread, null, null),
            0
        );
        assert_eq!(
            libc::ptrace(libc::PTRACE_INTERRUPT, input_thread, null, null),
            0
        );
        let mut status = 0;
        libc::waitpid(input_thread, &mut status, libc::__WALL);
        std::thread::sleep(Duration::from_millis(2500));
        libc::ptrace(libc::PTRACE_DETACH, input_thread, null, null);
    });
    let typed = send_virtual_keyboard_text_paced(
        cursor,
        u64::from(window),
        TEXT,
        TypePacing { delay_ms: 30 },
    );
    let error = typed.expect_err("the stalled server cannot confirm the text");
    // Retry at once, while the server may still list the unplugged keyboard.
    let retry = std::thread::spawn(move || {
        send_virtual_keyboard_text_paced(cursor, u64::from(window), "a", TypePacing::default())
    });
    stall.join().unwrap();
    let retried = retry.join().unwrap();
    assert!(
        format!("{error:#}").contains("the X server read"),
        "{error:#}"
    );

    // Mapping changes and key events reach this client in server order.
    // A key on a keycode the server has already restored is a wrong key.
    let mut restored: HashMap<u8, bool> = HashMap::new();
    let mut late = Vec::new();
    let mut pressed = 0;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match conn.poll_for_event().unwrap() {
            Some(Event::MappingNotify(notify)) if notify.request == Mapping::KEYBOARD => {
                for keycode in
                    notify.first_keycode..notify.first_keycode.saturating_add(notify.count)
                {
                    // The first change binds the spare keycode, the second restores it.
                    restored
                        .entry(keycode)
                        .and_modify(|done| *done = true)
                        .or_insert(false);
                }
            }
            Some(Event::KeyPress(press)) => {
                pressed += 1;
                // The retry's new keyboard taps X keycode 248 (evdev
                // KEY_UNKNOWN) to warm up; it is NoSymbol once restored.
                if press.detail != 248 && restored.get(&press.detail) == Some(&true) {
                    late.push(press.detail);
                }
            }
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    forget_master_pointer(cursor);
    let report = retried.expect("the retry types on a fresh keyboard");
    assert!(report.delivery_confirmed, "{report:?}");
    assert!(pressed > 0, "no key reached the window before the stall");
    assert_eq!(restored.len(), TEXT.chars().count(), "{restored:?}");
    assert!(
        restored.values().all(|done| *done),
        "a keycode was never restored: {restored:?}"
    );
    assert!(
        late.is_empty(),
        "keys read after their keycode was restored: {late:?}"
    );
}
