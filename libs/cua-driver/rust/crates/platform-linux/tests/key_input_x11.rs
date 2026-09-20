#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use platform_linux::input::{send_click, send_key, send_key_at, send_key_xtest, send_type_text};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

fn event_window(
    conn: &RustConnection,
    parent: Window,
    x: i16,
    y: i16,
    event_mask: EventMask,
) -> Result<Window> {
    let window = conn.generate_id()?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        parent,
        x,
        y,
        200,
        100,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .override_redirect(1)
            .event_mask(event_mask),
    )?
    .check()?;
    conn.map_window(window)?.check()?;
    let attributes = conn.get_window_attributes(window)?.reply()?;
    assert_eq!(attributes.map_state, MapState::VIEWABLE);
    assert_eq!(attributes.your_event_mask, event_mask);
    Ok(window)
}

fn input_window(conn: &RustConnection, parent: Window, x: i16, y: i16) -> Result<Window> {
    let window = event_window(
        conn,
        parent,
        x,
        y,
        EventMask::KEY_PRESS | EventMask::KEY_RELEASE,
    )?;
    let attributes = conn.get_window_attributes(window)?.reply()?;
    assert!(attributes.your_event_mask.contains(EventMask::KEY_PRESS));
    assert!(attributes.your_event_mask.contains(EventMask::KEY_RELEASE));
    Ok(window)
}

fn keyboard_events(conn: &RustConnection, expected: usize) -> Result<Vec<(bool, KeyPressEvent)>> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut events = Vec::new();
    while events.len() < expected {
        if Instant::now() >= deadline {
            bail!("received {} of {expected} keyboard events", events.len());
        }
        match conn.poll_for_event()? {
            Some(Event::KeyPress(event)) => events.push((true, event)),
            Some(Event::KeyRelease(event)) => events.push((false, event)),
            Some(Event::Error(error)) => bail!("X11 observer error: {error:?}"),
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    Ok(events)
}

fn assert_key(conn: &RustConnection, event: &KeyPressEvent, keysym: u32) -> Result<()> {
    let mapping = conn.get_keyboard_mapping(event.detail, 1)?.reply()?;
    assert!(
        mapping.keysyms.contains(&keysym),
        "expected keysym {keysym:#x}, received keycode {}",
        event.detail
    );
    Ok(())
}

#[test]
#[ignore = "requires an isolated X11 display"]
fn background_click_delivers_once_to_nearest_ancestor_selecting_button_events() -> Result<()> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let target = event_window(
        &conn,
        root,
        0,
        0,
        EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE,
    )?;
    let child = event_window(&conn, target, 20, 20, EventMask::NO_EVENT)?;
    let sentinel = input_window(&conn, root, 400, 0)?;
    conn.set_input_focus(InputFocus::PARENT, sentinel, x11rb::CURRENT_TIME)?;
    assert_eq!(conn.get_input_focus()?.reply()?.focus, sentinel);

    send_click(u64::from(target), 30, 30, 1, 1)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut events = Vec::new();
    while events.len() < 2 {
        if Instant::now() >= deadline {
            bail!("received {} of 2 button events", events.len());
        }
        match conn.poll_for_event()? {
            Some(Event::ButtonPress(event)) => events.push((true, event)),
            Some(Event::ButtonRelease(event)) => events.push((false, event)),
            Some(Event::Error(error)) => bail!("X11 observer error: {error:?}"),
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
    }

    assert!(events[0].0 && !events[1].0);
    for (_, event) in &events {
        assert_eq!(event.event, target);
        assert_ne!(event.event, child);
        assert_eq!((event.event_x, event.event_y), (30, 30));
        assert_ne!(
            event.response_type & 0x80,
            0,
            "expected XSendEvent delivery"
        );
    }
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        conn.poll_for_event()?.is_none(),
        "click delivered duplicate events"
    );
    assert_eq!(conn.get_input_focus()?.reply()?.focus, sentinel);
    Ok(())
}

/// Run on a disposable display: these tests change keyboard focus.
/// xvfb-run -a cargo test -p platform-linux --test key_input_x11 -- --ignored --test-threads=1
#[test]
#[ignore = "requires an isolated X11 display with XTEST"]
fn xtest_key_taps_have_a_delivered_hold_interval() -> Result<()> {
    let (conn, screen) = x11rb::connect(None)?;
    let window = input_window(&conn, conn.setup().roots[screen].root, 0, 0)?;
    conn.set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)?;
    assert_eq!(conn.get_input_focus()?.reply()?.focus, window);

    for modifiers in [vec![], vec!["ctrl"]] {
        send_key_xtest("up", &modifiers)?;
        let expected = if modifiers.is_empty() { 2 } else { 4 };
        let events = keyboard_events(&conn, expected)?;
        let press_index = usize::from(!modifiers.is_empty());
        let (pressed, press) = events[press_index];
        let (released, release) = events[press_index + 1];
        assert!(pressed && !released);
        assert_eq!(press.detail, release.detail);
        assert_eq!(press.event, window);
        assert_key(&conn, &press, 0xff52)?;

        // XTEST timestamps come from the server, so delayed test-thread
        // scheduling cannot make a buffered press/release pair look held.
        let held_ms = release.time.wrapping_sub(press.time);
        eprintln!("Up modifiers={modifiers:?}: delivered hold={held_ms} ms");
        assert!(
            held_ms >= 5,
            "key-down and key-up arrived only {held_ms} ms apart"
        );

        if !modifiers.is_empty() {
            assert!(events[0].0 && !events[3].0);
            assert_eq!(events[0].1.detail, events[3].1.detail);
            assert!(press.state.contains(KeyButMask::CONTROL));
            assert!(release.state.contains(KeyButMask::CONTROL));
        }
        let keymap = conn.query_keymap()?.reply()?;
        for (_, event) in &events {
            let keycode = usize::from(event.detail);
            assert_eq!(keymap.keys[keycode / 8] & (1 << (keycode % 8)), 0);
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires an isolated X11 display"]
fn background_keys_deliver_complete_sequences_without_changing_focus() -> Result<()> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let target = input_window(&conn, root, 0, 0)?;
    let child = input_window(&conn, target, 20, 20)?;
    let sentinel = input_window(&conn, root, 400, 0)?;
    conn.set_input_focus(InputFocus::PARENT, sentinel, x11rb::CURRENT_TIME)?;
    assert_eq!(conn.get_input_focus()?.reply()?.focus, sentinel);

    for coordinate_target in [false, true] {
        for modifiers in [vec![], vec!["ctrl"]] {
            let destination = if coordinate_target { child } else { target };
            if coordinate_target {
                send_key_at(u64::from(target), 30, 30, "up", &modifiers)?;
            } else {
                send_key(u64::from(target), "up", &modifiers)?;
            }
            let modified = !modifiers.is_empty();
            let events = keyboard_events(&conn, if modified { 4 } else { 2 })?;
            let press_index = usize::from(modified);
            for (_, event) in &events {
                assert_eq!(event.event, destination);
                assert_ne!(
                    event.response_type & 0x80,
                    0,
                    "expected XSendEvent delivery"
                );
            }
            assert!(events[press_index].0 && !events[press_index + 1].0);
            assert_key(&conn, &events[press_index].1, 0xff52)?;
            assert_eq!(
                events[press_index].1.detail,
                events[press_index + 1].1.detail
            );
            if modified {
                assert!(events[0].0 && !events[3].0);
                assert_key(&conn, &events[0].1, 0xffe3)?;
                assert_eq!(events[0].1.detail, events[3].1.detail);
                assert!(events[1].1.state.contains(KeyButMask::CONTROL));
                assert!(events[2].1.state.contains(KeyButMask::CONTROL));
            }
            assert_eq!(conn.get_input_focus()?.reply()?.focus, sentinel);
        }
    }

    send_type_text(u64::from(target), "az")?;
    let events = keyboard_events(&conn, 4)?;
    for (pair, keysym) in events.chunks_exact(2).zip([u32::from('a'), u32::from('z')]) {
        assert!(pair[0].0 && !pair[1].0);
        assert_eq!(pair[0].1.event, target);
        assert_eq!(pair[1].1.event, target);
        assert_eq!(pair[0].1.detail, pair[1].1.detail);
        assert_key(&conn, &pair[0].1, keysym)?;
    }
    assert_eq!(conn.get_input_focus()?.reply()?.focus, sentinel);
    Ok(())
}
