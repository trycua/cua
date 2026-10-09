#![cfg(target_os = "linux")]

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use platform_linux::input::{
    send_click, send_key, send_key_at, send_key_xtest, send_type_text, send_type_text_xtest,
};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// Connects to the test display while one connection stays open for the
/// whole test binary. An X server started without `-noreset` (plain
/// `xvfb-run`) resets when its last client disconnects and drops a connection
/// that arrives during the reset, so without this the connection setup of the
/// test that runs after another one closed everything could read EOF or
/// ECONNRESET.
fn connect() -> Result<(RustConnection, usize)> {
    static KEEP_DISPLAY: OnceLock<RustConnection> = OnceLock::new();
    if KEEP_DISPLAY.get().is_none() {
        let _ = KEEP_DISPLAY.set(x11rb::connect(None)?.0);
    }
    Ok(x11rb::connect(None)?)
}

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

fn assert_no_button_events(conn: &RustConnection, label: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline {
        match conn.poll_for_event()? {
            Some(Event::ButtonPress(event)) => {
                bail!("{label} unexpectedly received ButtonPress: {event:?}")
            }
            Some(Event::ButtonRelease(event)) => {
                bail!("{label} unexpectedly received ButtonRelease: {event:?}")
            }
            Some(Event::Error(error)) => bail!("{label} X11 error: {error:?}"),
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    Ok(())
}

fn spare_keycodes(conn: &RustConnection) -> Result<Vec<u8>> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let mapping = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
    let per = usize::from(mapping.keysyms_per_keycode);
    Ok(mapping
        .keysyms
        .chunks(per)
        .enumerate()
        .filter(|(_, keysyms)| keysyms.iter().all(|&keysym| keysym == 0))
        .map(|(index, _)| min + index as u8)
        .collect())
}

fn focused_input_window(conn: &RustConnection, screen: usize) -> Result<Window> {
    let window = input_window(conn, conn.setup().roots[screen].root, 0, 0)?;
    conn.set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)?;
    assert_eq!(conn.get_input_focus()?.reply()?.focus, window);
    Ok(window)
}

#[test]
#[ignore = "requires an isolated X11 display"]
fn background_click_delivers_complete_sequence_to_press_recipient_without_focus_change(
) -> Result<()> {
    let (owner, screen) = connect()?;
    let root = owner.setup().roots[screen].root;
    let target = event_window(&owner, root, 0, 0, EventMask::NO_EVENT)?;
    let leaf = event_window(&owner, target, 20, 20, EventMask::NO_EVENT)?;
    let sentinel = event_window(&owner, root, 400, 0, EventMask::NO_EVENT)?;

    let (press_observer, _) = connect()?;
    press_observer
        .change_window_attributes(
            target,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::BUTTON_PRESS),
        )?
        .check()?;
    let (release_observer, _) = connect()?;
    release_observer
        .change_window_attributes(
            leaf,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::BUTTON_RELEASE),
        )?
        .check()?;

    owner.set_input_focus(InputFocus::PARENT, sentinel, x11rb::CURRENT_TIME)?;
    assert_eq!(owner.get_input_focus()?.reply()?.focus, sentinel);

    send_click(u64::from(target), 30, 30, 1, 1)?;

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut events = Vec::new();
    while events.len() < 2 {
        if Instant::now() >= deadline {
            bail!("received {} of 2 button events", events.len());
        }
        match press_observer.poll_for_event()? {
            Some(Event::ButtonPress(event)) => events.push((true, event)),
            Some(Event::ButtonRelease(event)) => events.push((false, event)),
            Some(Event::Error(error)) => bail!("X11 press observer error: {error:?}"),
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
    }

    assert!(events[0].0 && !events[1].0);
    for (_, event) in &events {
        assert_eq!(event.event, target);
        assert_ne!(event.event, leaf);
        assert_eq!((event.event_x, event.event_y), (30, 30));
        assert_ne!(
            event.response_type & 0x80,
            0,
            "expected XSendEvent delivery"
        );
    }
    assert_no_button_events(&release_observer, "release observer")?;
    assert_no_button_events(&owner, "owner")?;
    assert_eq!(owner.get_input_focus()?.reply()?.focus, sentinel);
    Ok(())
}

/// Run on a disposable display: these tests change keyboard focus.
/// xvfb-run -a cargo test -p platform-linux --test key_input_x11 -- --ignored --test-threads=1
#[test]
#[ignore = "requires an isolated X11 display with XTEST"]
fn xtest_key_taps_have_a_delivered_hold_interval() -> Result<()> {
    let (conn, screen) = connect()?;
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
    let (conn, screen) = connect()?;
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

/// With no spare keycode left, a character missing from the keymap cannot be
/// typed, and type_text must say so instead of reporting success.
#[test]
#[ignore = "requires an isolated X11 display with XTEST"]
fn xtest_text_fails_when_no_spare_keycode_is_left() -> Result<()> {
    let (conn, screen) = connect()?;
    focused_input_window(&conn, screen)?;
    let spare = spare_keycodes(&conn)?;
    let per = conn
        .get_keyboard_mapping(spare[0], 1)?
        .reply()?
        .keysyms_per_keycode;
    let bind = |keysym: u32| -> Result<()> {
        for &keycode in &spare {
            conn.change_keyboard_mapping(1, keycode, per, &vec![keysym; usize::from(per)])?;
        }
        conn.get_input_focus()?.reply()?;
        Ok(())
    };
    bind(0x7e1)?;
    let result = send_type_text_xtest("\u{4f60}");
    bind(0)?;
    let error = result.expect_err("typing without a spare keycode reported success");
    assert!(
        error.to_string().starts_with("typed 0 of 1 characters"),
        "{error:#}"
    );
    Ok(())
}

/// Characters of one text that are missing from the keymap must not share a
/// borrowed keycode: rebinding it while the client still translates an earlier
/// key changes what the client reads. A repeated character reuses its own.
#[test]
#[ignore = "requires an isolated X11 display with XTEST"]
fn xtest_text_gives_each_missing_character_its_own_keycode() -> Result<()> {
    let (conn, screen) = connect()?;
    focused_input_window(&conn, screen)?;
    let spare = spare_keycodes(&conn)?;
    assert!(
        spare.len() >= 2,
        "the test display has fewer than 2 spare keycodes"
    );
    send_type_text_xtest("\u{4f60}\u{597d}\u{4f60}")?;
    let presses: Vec<u8> = keyboard_events(&conn, 6)?
        .into_iter()
        .filter(|(pressed, _)| *pressed)
        .map(|(_, event)| event.detail)
        .collect();
    assert!(
        presses.iter().all(|keycode| spare.contains(keycode)),
        "{presses:?}"
    );
    assert_ne!(presses[0], presses[1], "{presses:?}");
    assert_eq!(presses[0], presses[2], "{presses:?}");
    Ok(())
}

/// A borrowed keycode is restored only after the focused client answers a
/// `_NET_WM_PING` sent after its keys, so a client that is slow to read its
/// events still translates them under the borrowed binding.
#[test]
#[ignore = "requires an isolated X11 display with XTEST"]
fn xtest_text_restores_a_borrowed_keycode_only_after_the_client_answers_a_ping() -> Result<()> {
    let (conn, screen) = connect()?;
    let root = conn.setup().roots[screen].root;
    let window = focused_input_window(&conn, screen)?;
    let atom = |name: &str| -> Result<Atom> {
        Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
    };
    let (protocols, ping, active) = (
        atom("WM_PROTOCOLS")?,
        atom("_NET_WM_PING")?,
        atom("_NET_ACTIVE_WINDOW")?,
    );
    conn.change_property32(
        PropMode::REPLACE,
        window,
        protocols,
        AtomEnum::ATOM,
        &[ping],
    )?;
    conn.change_property32(PropMode::REPLACE, root, active, AtomEnum::WINDOW, &[window])?;
    conn.get_input_focus()?.reply()?;
    let before = spare_keycodes(&conn)?;

    let typing = std::thread::spawn(|| send_type_text_xtest("\u{4f60}"));
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut pings, mut pressed) = (0, None);
    while !typing.is_finished() && Instant::now() < deadline {
        match conn.poll_for_event()? {
            Some(Event::KeyPress(event)) => pressed = Some(event.detail),
            Some(Event::ClientMessage(event))
                if event.type_ == protocols && event.data.as_data32()[0] == ping =>
            {
                pings += 1;
                std::thread::sleep(Duration::from_millis(200));
                if let Some(keycode) = pressed {
                    let keysyms = conn.get_keyboard_mapping(keycode, 1)?.reply()?.keysyms;
                    assert!(
                        keysyms.contains(&0x0100_4f60),
                        "keycode {keycode} restored before the pong"
                    );
                }
                let pong = ClientMessageEvent::new(32, root, protocols, event.data);
                conn.send_event(
                    false,
                    root,
                    EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
                    pong,
                )?;
                conn.flush()?;
            }
            Some(Event::Error(error)) => bail!("X11 observer error: {error:?}"),
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    let result = typing.join().expect("typing thread panicked");
    conn.delete_property(root, active)?;
    result?;
    assert!(pressed.is_some(), "the key never arrived");
    assert_eq!(
        pings, 2,
        "expected a ping after the binding and one before the restore"
    );
    assert_eq!(spare_keycodes(&conn)?, before);
    Ok(())
}
