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

/// Both tests stop the same thread of the same server.
static SERVER: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The thread of the X server that reads evdev. While it is stopped the
/// server still answers requests, as it does when it is short of CPU.
fn input_thread() -> libc::pid_t {
    let server = std::env::var("CUA_TEST_XORG_PID")
        .expect("CUA_TEST_XORG_PID names the X server whose input thread is stalled");
    std::fs::read_dir(format!("/proc/{server}/task"))
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
        .unwrap()
}

fn key_window(events: EventMask) -> (x11rb::rust_connection::RustConnection, Window) {
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
            .event_mask(events),
    )
    .unwrap();
    conn.map_window(window).unwrap();
    conn.get_input_focus().unwrap().reply().unwrap();
    (conn, window)
}

#[test]
#[ignore = "requires a real Xorg (CUA_TEST_XORG_PID) with libinput hotplug and /dev/uinput"]
fn a_key_the_server_reads_down_but_not_up_is_not_repeated() {
    let _server = SERVER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let input_thread = input_thread();
    let (conn, window) = key_window(EventMask::KEY_PRESS | EventMask::KEY_RELEASE);
    let cursor = "virtual-keyboard-xorg-repeat";
    send_virtual_keyboard_text_paced(cursor, u64::from(window), "a", TypePacing::default())
        .unwrap();
    while conn.poll_for_event().unwrap().is_some() {}

    // Stop the input thread as soon as the fifth key is down. Its release
    // follows 10 ms later, so the server holds that key for the whole stall.
    let watch = std::thread::spawn(move || unsafe {
        let null = std::ptr::null_mut::<libc::c_void>();
        assert_eq!(
            libc::ptrace(libc::PTRACE_SEIZE, input_thread, null, null),
            0
        );
        let mut keys = Vec::new();
        while keys.iter().filter(|(down, _, _)| *down).count() < 5 {
            match conn.wait_for_event().unwrap() {
                Event::KeyPress(key) => keys.push((true, key.detail, key.time)),
                Event::KeyRelease(key) => keys.push((false, key.detail, key.time)),
                _ => {}
            }
        }
        assert_eq!(
            libc::ptrace(libc::PTRACE_INTERRUPT, input_thread, null, null),
            0
        );
        let mut status = 0;
        libc::waitpid(input_thread, &mut status, libc::__WALL);
        let stalled = Instant::now();
        let mut running = false;
        while stalled.elapsed() < Duration::from_millis(3500) {
            if !running && stalled.elapsed() >= Duration::from_millis(2500) {
                libc::ptrace(libc::PTRACE_DETACH, input_thread, null, null);
                running = true;
            }
            match conn.poll_for_event().unwrap() {
                Some(Event::KeyPress(key)) => keys.push((true, key.detail, key.time)),
                Some(Event::KeyRelease(key)) => keys.push((false, key.detail, key.time)),
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        keys
    });
    let typed = send_virtual_keyboard_text_paced(
        cursor,
        u64::from(window),
        "abcdefghijklmnopqrstuvwxyz",
        TypePacing { delay_ms: 30 },
    );
    let keys = watch.join().unwrap();
    forget_master_pointer(cursor);
    typed.expect_err("the stalled server cannot confirm the text");

    let down: Vec<u8> = keys
        .iter()
        .filter(|(down, _, _)| *down)
        .map(|key| key.1)
        .collect();
    let held = down[4];
    let times = |want: bool| {
        keys.iter()
            .filter(move |(down, keycode, _)| *down == want && *keycode == held)
            .map(|key| key.2)
    };
    let held_for = times(false)
        .next_back()
        .expect("the held key is released when its keyboard is unplugged")
        .wrapping_sub(times(true).next().unwrap());
    assert!(
        held_for >= 1000,
        "the stall missed the key: it was down for {held_for} ms"
    );
    // Every character of the text is on its own key, so a second press of
    // any key is one the driver never sent.
    let repeats = down.len() - down.iter().collect::<std::collections::HashSet<_>>().len();
    assert_eq!(repeats, 0, "the server repeated a held key: {down:?}");
}

#[test]
#[ignore = "requires a real Xorg (CUA_TEST_XORG_PID) with libinput hotplug and /dev/uinput"]
fn a_keyboard_whose_warm_up_key_the_server_never_reads_is_not_kept() {
    let _server = SERVER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let input_thread = input_thread();
    let (conn, window) = key_window(EventMask::KEY_PRESS);
    let cursor = "virtual-keyboard-xorg-warm-up";
    let null = std::ptr::null_mut::<libc::c_void>();
    // The server lists the new keyboard, but reads nothing from it.
    unsafe {
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
    }
    let typed =
        send_virtual_keyboard_text_paced(cursor, u64::from(window), "a", TypePacing::default());
    unsafe { libc::ptrace(libc::PTRACE_DETACH, input_thread, null, null) };
    let retried =
        send_virtual_keyboard_text_paced(cursor, u64::from(window), "a", TypePacing::default());
    forget_master_pointer(cursor);

    let error = typed.expect_err("the server never read the warm-up key");
    assert!(format!("{error:#}").contains("warm-up key"), "{error:#}");
    let report = retried.expect("the retry plugs in a new keyboard");
    assert!(report.delivery_confirmed, "{report:?}");
    std::thread::sleep(Duration::from_millis(300));
    let mut pressed = 0;
    while let Some(event) = conn.poll_for_event().unwrap() {
        pressed += usize::from(matches!(event, Event::KeyPress(_)));
    }
    assert_eq!(pressed, 1, "only the retry's key reaches the window");
}

#[test]
#[ignore = "requires a real Xorg (CUA_TEST_XORG_PID) with libinput hotplug and /dev/uinput"]
fn a_stalled_server_never_reads_borrowed_keys_after_their_keycodes_are_restored() {
    let _server = SERVER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let input_thread = input_thread();
    let (conn, window) = key_window(EventMask::KEY_PRESS);
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
                if restored.get(&press.detail) == Some(&true) {
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
