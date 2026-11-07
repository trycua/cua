// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Linux/X11 `type_text` with `delivery_mode:"foreground"` activates the
//! target, types, then gives the active window back to the one the user had
//! (CUA-3663).
//!
//! The test process maps two titled toplevels: `prior`, which it activates
//! first, and `target`, which listens for key presses. The oracles are the
//! target connection's own KeyPress events (fixture state) and the window
//! manager's `_NET_ACTIVE_WINDOW` before and after the call (focus).
//!
//! #[ignore] (needs an X11 display with a window manager). Run:
//!   cargo test -p cua-driver-e2e --test x11_foreground_restore_linux_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Observation, OracleKind,
    Scope, Targeting,
};
use cua_driver_testkit::{Driver, McpDriver};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Window,
    WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

struct Desktop {
    conn: RustConnection,
    root: Window,
    net_active_window: u32,
}

impl Desktop {
    fn connect() -> Self {
        let (conn, screen_num) =
            RustConnection::connect(None).expect("connect to the X11 display under test");
        let root = conn.setup().roots[screen_num].root;
        let net_active_window = conn
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")
            .expect("intern _NET_ACTIVE_WINDOW")
            .reply()
            .expect("intern _NET_ACTIVE_WINDOW reply")
            .atom;
        Self {
            conn,
            root,
            net_active_window,
        }
    }

    fn map(&self, title: &str, x: i16) -> Window {
        let screen = &self.conn.setup().roots[0];
        let window = self.conn.generate_id().expect("allocate X11 window id");
        self.conn
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                self.root,
                x,
                80,
                360,
                240,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &CreateWindowAux::new()
                    .background_pixel(screen.white_pixel)
                    .event_mask(EventMask::KEY_PRESS | EventMask::EXPOSURE),
            )
            .expect("create X11 toplevel");
        self.conn
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                title.as_bytes(),
            )
            .expect("set WM_NAME");
        let net_wm_pid = self
            .conn
            .intern_atom(false, b"_NET_WM_PID")
            .expect("intern _NET_WM_PID")
            .reply()
            .expect("intern _NET_WM_PID reply")
            .atom;
        self.conn
            .change_property32(
                PropMode::REPLACE,
                window,
                net_wm_pid,
                AtomEnum::CARDINAL,
                &[std::process::id()],
            )
            .expect("set _NET_WM_PID");
        self.conn.map_window(window).expect("map X11 toplevel");
        self.conn.flush().expect("flush X11 requests");
        window
    }

    fn active(&self) -> Option<Window> {
        self.conn
            .get_property(
                false,
                self.root,
                self.net_active_window,
                AtomEnum::WINDOW,
                0,
                1,
            )
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .next()
            .filter(|w| *w != 0)
    }

    /// Ask the window manager to activate `window` as a pager would.
    fn activate(&self, window: Window) {
        let event = ClientMessageEvent::new(32, window, self.net_active_window, [2, 0, 0, 0, 0]);
        self.conn
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .expect("send _NET_ACTIVE_WINDOW");
        self.conn.flush().expect("flush activation");
    }

    fn wait_active(&self, window: Window) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.active() == Some(window) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn key_presses(&self, window: Window) -> usize {
        let mut presses = 0;
        while let Ok(Some(event)) = self.conn.poll_for_event() {
            if matches!(event, Event::KeyPress(press) if press.event == window) {
                presses += 1;
            }
        }
        presses
    }
}

#[test]
#[ignore]
fn x11_foreground_type_text_restores_the_prior_active_window() {
    if std::env::var_os("DISPLAY").is_none() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some() {
            panic!("the X11 foreground restore case requires a native X11 session");
        }
        eprintln!("[x11-foreground-restore] not a native X11 session; skipping");
        return;
    }
    let cell_id = "linux-xlib-foreground-restore-type-text-px-foreground";
    let case = CaseSpec::delivered(
        cell_id,
        "x11-bare-client",
        "xlib",
        "type_text",
        Targeting::Px,
        Delivery::Foreground,
        Scope::Window,
        DriverRoute::LinuxXTest,
        vec![OracleKind::FixtureState, OracleKind::Focus],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_named(cell_id).expect("start source-built Linux driver");
        *evidence = recording_evidence(driver.recording_dir());
        let desktop = Desktop::connect();
        let target = desktop.map("CuaForegroundRestoreTarget", 80);
        let prior = desktop.map("CuaForegroundRestorePrior", 520);
        desktop.activate(prior);
        assert!(
            desktop.wait_active(prior),
            "fixture precondition: the prior window never became active"
        );
        desktop.key_presses(target);

        driver.start_behavior_recording();
        let typed = driver.call(
            "type_text",
            serde_json::json!({
                "pid": std::process::id(),
                "window_id": u64::from(target),
                "text": "abc",
                "delivery_mode": "foreground"
            }),
        );
        assert!(
            !typed.is_error(),
            "foreground type_text failed: {}",
            typed.text()
        );

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut presses = 0;
        while presses < 3 && Instant::now() < deadline {
            presses += desktop.key_presses(target);
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            presses >= 3,
            "the target received {presses} of 3 key presses: {}",
            typed.text()
        );
        assert_eq!(
            desktop.active(),
            Some(prior),
            "foreground type_text left the active window on the target \
             instead of the prior window: {}",
            typed.text()
        );
        Observation::delivered_with_fixture_state(vec![OracleKind::Focus])
    });
}
