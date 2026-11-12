// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! trycua/cua#7325: ending a session must never remove an XI2 master device.
//! Every running Qt 6.5+ client segfaults on a master removal (QTBUG-146876),
//! so session pairs are pooled: freed at session end, reused by the next one.
//!
//! Needs a real Xorg with libinput hotplug and a writable `/dev/uinput`; Xvfb
//! has neither. Set `CUA_TEST_QT6_PYTHON` to a Python with PySide6 6.5+ to
//! also check that a live Qt 6 window survives (it is killed without the fix):
//!
//! `DISPLAY=:0 CUA_TEST_QT6_PYTHON=~/venv/bin/python cargo test -p platform-linux --test mpx_pool_xorg -- --ignored --nocapture`

#![cfg(target_os = "linux")]

use std::process::{Child, Command};
use std::time::Duration;

use platform_linux::input::{forget_master_pointer, send_virtual_keyboard_text_paced, TypePacing};
use x11rb::connection::Connection;
use x11rb::protocol::xinput::{self, ConnectionExt as _, DeviceType, HierarchyMask};
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

const QT_WINDOW: &str = r#"
import sys
from PySide6.QtWidgets import QApplication, QLineEdit
app = QApplication(sys.argv)
w = QLineEdit(); w.setWindowTitle("cua-7325"); w.show()
app.exec()
"#;

/// `(device id, name)` of every master pointer.
fn master_pointers(conn: &RustConnection) -> Vec<(u16, String)> {
    conn.xinput_xi_query_device(xinput::Device::ALL_MASTER)
        .unwrap()
        .reply()
        .unwrap()
        .infos
        .into_iter()
        .filter(|info| info.type_ == DeviceType::MASTER_POINTER)
        .map(|info| {
            (
                info.deviceid,
                String::from_utf8_lossy(&info.name).into_owned(),
            )
        })
        .collect()
}

fn holder_mark(conn: &RustConnection, device: u16) -> Option<Vec<u8>> {
    let atom = conn
        .intern_atom(true, b"_CUA_MPX_HOLDER")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let reply = conn
        .xinput_xi_get_property(device, false, atom, u32::from(xproto::AtomEnum::ANY), 0, 64)
        .unwrap()
        .reply()
        .unwrap();
    (reply.type_ != 0).then(|| match reply.items {
        xinput::XIGetPropertyItems::Data8(bytes) => bytes,
        _ => panic!("holder mark is not a string"),
    })
}

fn target_window(conn: &RustConnection, root: u32) -> u32 {
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
        xproto::WindowClass::INPUT_OUTPUT,
        0,
        &xproto::CreateWindowAux::new()
            .override_redirect(1)
            .event_mask(xproto::EventMask::KEY_PRESS),
    )
    .unwrap();
    conn.map_window(window).unwrap();
    conn.get_input_focus().unwrap().reply().unwrap();
    window
}

fn qt_window() -> Option<Child> {
    let python = std::env::var("CUA_TEST_QT6_PYTHON").ok()?;
    let child = Command::new(python)
        .args(["-c", QT_WINDOW])
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(3));
    Some(child)
}

#[test]
#[ignore = "requires a real Xorg with libinput hotplug and a writable /dev/uinput"]
fn ending_sessions_frees_and_reuses_master_pairs_without_removing_any() {
    let mut qt = qt_window();
    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    conn.xinput_xi_query_version(2, 3).unwrap().reply().unwrap();
    conn.xinput_xi_select_events(
        root,
        &[xinput::EventMask {
            deviceid: xinput::Device::ALL.into(),
            mask: vec![xinput::XIEventMask::HIERARCHY],
        }],
    )
    .unwrap();
    let window = target_window(&conn, root);
    let before = master_pointers(&conn);

    let mut pairs = Vec::new();
    for session in ["mpx-pool-a", "mpx-pool-b", "mpx-pool-a"] {
        send_virtual_keyboard_text_paced(session, u64::from(window), "a", TypePacing::default())
            .unwrap();
        let during = master_pointers(&conn);
        let added: Vec<_> = during
            .iter()
            .filter(|p| !before.contains(p))
            .cloned()
            .collect();
        forget_master_pointer(session);
        let after = master_pointers(&conn);
        assert_eq!(
            after, during,
            "ending '{session}' changed the master pointers"
        );
        pairs.push(added);
    }

    // The first session added at most one pair; later sessions took it again.
    assert!(pairs[0].len() <= 1, "one session added {:?}", pairs[0]);
    assert_eq!(pairs[1], pairs[0]);
    assert_eq!(pairs[2], pairs[0]);
    for (device, name) in master_pointers(&conn) {
        if name.starts_with("CUA ") {
            assert_eq!(
                holder_mark(&conn, device).as_deref(),
                Some(&b""[..]),
                "{name} not freed"
            );
        }
    }

    conn.get_input_focus().unwrap().reply().unwrap();
    while let Some(event) = conn.poll_for_event().unwrap() {
        if let Event::XinputHierarchy(event) = event {
            assert!(
                !event.flags.contains(HierarchyMask::MASTER_REMOVED),
                "a master device was removed: {event:?}"
            );
        }
    }

    if let Some(qt) = qt.as_mut() {
        std::thread::sleep(Duration::from_secs(1));
        assert!(
            qt.try_wait().unwrap().is_none(),
            "the Qt 6 window died when a session ended"
        );
        qt.kill().unwrap();
        qt.wait().unwrap();
    }
}
