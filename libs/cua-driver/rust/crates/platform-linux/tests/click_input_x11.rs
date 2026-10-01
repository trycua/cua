// SPDX-License-Identifier: MIT

#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use platform_linux::input::{
    send_button_down, send_button_up, send_click_with_modifiers,
    send_click_xtest_desktop_with_modifiers,
};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

fn input_window(conn: &RustConnection, parent: Window, x: i16) -> Result<Window> {
    let window = conn.generate_id()?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        parent,
        x,
        0,
        200,
        120,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .override_redirect(1)
            .event_mask(EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE),
    )?
    .check()?;
    conn.map_window(window)?.check()?;
    assert_eq!(
        conn.get_window_attributes(window)?.reply()?.map_state,
        MapState::VIEWABLE
    );
    Ok(window)
}

fn button_events(
    conn: &RustConnection,
    expected: usize,
) -> Result<Vec<(bool, ButtonPressEvent, Instant)>> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut events = Vec::new();
    while events.len() < expected {
        if Instant::now() >= deadline {
            bail!("received {} of {expected} button events", events.len());
        }
        match conn.poll_for_event()? {
            Some(Event::ButtonPress(event)) => events.push((true, event, Instant::now())),
            Some(Event::ButtonRelease(event)) => events.push((false, event, Instant::now())),
            Some(Event::Error(error)) => bail!("X11 observer error: {error:?}"),
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    Ok(events)
}

fn assert_pair(events: &[(bool, ButtonPressEvent, Instant)], target: Window, modified: bool) {
    assert!(events[0].0 && !events[1].0);
    let press = events[0].1;
    let release = events[1].1;
    assert_eq!(press.detail, 1);
    assert_eq!(release.detail, 1);
    assert_eq!(press.event, target);
    assert_eq!(release.event, target);
    assert!(!press.state.contains(KeyButMask::BUTTON1));
    assert!(release.state.contains(KeyButMask::BUTTON1));
    assert_eq!(press.state.contains(KeyButMask::CONTROL), modified);
    assert_eq!(release.state.contains(KeyButMask::CONTROL), modified);
}

/// These tests need a disposable display because XTest moves the real pointer.
/// xvfb-run -a cargo test -p platform-linux --test click_input_x11 -- --ignored --test-threads=1
#[test]
#[ignore = "requires an isolated X11 display with XTEST"]
fn desktop_clicks_deliver_a_hold_and_release_modifiers() -> Result<()> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let target = input_window(&conn, root, 0)?;
    conn.set_input_focus(InputFocus::PARENT, target, x11rb::CURRENT_TIME)?
        .check()?;

    for count in [1, 2, 3] {
        for modifiers in [vec![], vec!["ctrl"]] {
            send_click_xtest_desktop_with_modifiers(50, 50, 1, count, &modifiers)?;
            let events = button_events(&conn, count * 2)?;
            for pair in events.chunks_exact(2) {
                assert_pair(pair, target, !modifiers.is_empty());
                assert_eq!(pair[0].1.response_type & 0x80, 0);
                // Server timestamps cannot turn buffered transitions into a hold
                // merely because the observer thread was scheduled late.
                let held_ms = pair[1].1.time.wrapping_sub(pair[0].1.time);
                eprintln!("desktop count={count} modifiers={modifiers:?}: hold={held_ms} ms");
                assert!(held_ms >= 30, "desktop click held only {held_ms} ms");
            }
            for adjacent in events.chunks_exact(2).collect::<Vec<_>>().windows(2) {
                let gap_ms = adjacent[1][0].1.time.wrapping_sub(adjacent[0][1].1.time);
                assert!(gap_ms >= 40, "multi-click gap was only {gap_ms} ms");
            }
            let pointer = conn.query_pointer(root)?.reply()?;
            assert!(!pointer
                .mask
                .intersects(KeyButMask::BUTTON1 | KeyButMask::CONTROL));
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires an isolated X11 display"]
fn background_clicks_deliver_spaced_pairs_without_changing_focus() -> Result<()> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let target = input_window(&conn, root, 0)?;
    let sentinel = input_window(&conn, root, 300)?;
    conn.set_input_focus(InputFocus::PARENT, sentinel, x11rb::CURRENT_TIME)?
        .check()?;

    for modifiers in [vec![], vec!["ctrl"]] {
        // XSendEvent preserves the supplied timestamp (CURRENT_TIME), so observe
        // arrivals concurrently rather than claiming those zero timestamps prove timing.
        let events = std::thread::scope(|scope| -> Result<_> {
            let sender = scope
                .spawn(|| send_click_with_modifiers(u64::from(target), 50, 50, 5, 1, &modifiers));
            let events = button_events(&conn, 10);
            sender.join().expect("click sender panicked")?;
            events
        })?;
        let mut holds = Vec::new();
        for pair in events.chunks_exact(2) {
            assert_pair(pair, target, !modifiers.is_empty());
            assert_ne!(pair[0].1.response_type & 0x80, 0);
            holds.push(pair[1].2.duration_since(pair[0].2).as_millis());
        }
        eprintln!("background modifiers={modifiers:?}: observed holds={holds:?} ms");
        // Allow an occasional delayed observer wakeup; a buffered press/release
        // pair remains near zero for every click on the original implementation.
        holds.sort_unstable();
        assert!(
            holds[2] >= 20,
            "background click median hold was only {} ms",
            holds[2]
        );
        assert_eq!(conn.get_input_focus()?.reply()?.focus, sentinel);
    }
    Ok(())
}

#[test]
#[ignore = "requires an isolated X11 display"]
fn separate_background_button_calls_deliver_ordered_pairs() -> Result<()> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let target = input_window(&conn, root, 0)?;
    let sentinel = input_window(&conn, root, 300)?;
    conn.set_input_focus(InputFocus::PARENT, sentinel, x11rb::CURRENT_TIME)?
        .check()?;

    for _ in 0..10 {
        send_button_down(u64::from(target), 50, 50, 1)?;
        send_button_up(u64::from(target), 50, 50, 1)?;
        let events = button_events(&conn, 2)?;
        assert_pair(&events, target, false);
        assert_eq!(conn.get_input_focus()?.reply()?.focus, sentinel);
    }
    Ok(())
}

#[test]
#[ignore = "requires an isolated X11 display with XTEST"]
fn rejected_desktop_button_releases_preceding_modifiers() -> Result<()> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let target = input_window(&conn, root, 0)?;
    conn.set_input_focus(InputFocus::PARENT, target, x11rb::CURRENT_TIME)?
        .check()?;

    // A real server rejection occurs after Ctrl-down has already been queued.
    // This exercises partial acquisition without a production injection seam.
    assert!(send_click_xtest_desktop_with_modifiers(50, 50, 0, 1, &["ctrl"]).is_err());
    let pointer = conn.query_pointer(root)?.reply()?;
    assert!(!pointer
        .mask
        .intersects(KeyButMask::BUTTON1 | KeyButMask::CONTROL));
    Ok(())
}
