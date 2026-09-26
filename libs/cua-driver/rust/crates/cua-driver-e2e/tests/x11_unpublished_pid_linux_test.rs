//! Linux/X11 toplevels that never publish `_NET_WM_PID` (Tk, many Java/AWT
//! builds, Wine, legacy Xlib/Xt clients) must still be addressable by
//! `pid` + `window_id` (#3894).
//!
//! The test process is itself the bare Xlib-style client: it maps a titled
//! toplevel through x11rb without `_NET_WM_PID` or `WM_CLIENT_MACHINE`, so
//! the only way the driver can attribute it is the X-Resource
//! `LocalClientPID` of the connection that created it. The oracles are
//! protocol (`list_windows` reports this process's pid, a pid-filtered list
//! finds the window, and `get_window_state` accepts the pair) and fixture
//! state (the window's own connection receives the ButtonPress of a
//! window-scoped background click).
//!
//! #[ignore] (needs an X11 display with a window manager). Run:
//!   cargo test -p cua-driver-e2e --test x11_unpublished_pid_linux_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Observation, OracleKind,
    Scope, Targeting,
};
use cua_driver_testkit::{Driver, McpDriver};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

struct BareClient {
    conn: RustConnection,
    window: u32,
    title: String,
}

impl BareClient {
    fn map() -> Self {
        let (conn, screen_num) =
            RustConnection::connect(None).expect("connect to the X11 display under test");
        let screen = &conn.setup().roots[screen_num];
        let window = conn.generate_id().expect("allocate X11 window id");
        let title = format!("CuaUnpublishedPid-{}", std::process::id());
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            screen.root,
            80,
            80,
            480,
            320,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new()
                .background_pixel(screen.white_pixel)
                .event_mask(EventMask::BUTTON_PRESS | EventMask::EXPOSURE),
        )
        .expect("create bare X11 toplevel");
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        )
        .expect("set WM_NAME");
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"cua-unpublished-pid\0CuaUnpublishedPid\0",
        )
        .expect("set WM_CLASS");
        conn.map_window(window).expect("map bare X11 toplevel");
        conn.flush().expect("flush X11 requests");
        Self {
            conn,
            window,
            title,
        }
    }

    /// Precondition: the client really publishes no `_NET_WM_PID`.
    fn publishes_net_wm_pid(&self) -> bool {
        let atom = self
            .conn
            .intern_atom(false, b"_NET_WM_PID")
            .expect("intern _NET_WM_PID")
            .reply()
            .expect("intern _NET_WM_PID reply")
            .atom;
        let reply = self
            .conn
            .get_property(false, self.window, atom, AtomEnum::ANY, 0, 1)
            .expect("read _NET_WM_PID")
            .reply()
            .expect("read _NET_WM_PID reply");
        reply.type_ != x11rb::NONE
    }

    fn drain_button_presses(&self) -> usize {
        let mut presses = 0;
        while let Ok(Some(event)) = self.conn.poll_for_event() {
            if matches!(event, Event::ButtonPress(press) if press.event == self.window) {
                presses += 1;
            }
        }
        presses
    }
}

fn listed_record(
    driver: &mut McpDriver,
    args: serde_json::Value,
    window: u64,
) -> Option<serde_json::Value> {
    driver.call("list_windows", args).structured()["windows"]
        .as_array()?
        .iter()
        .find(|w| w["window_id"].as_u64() == Some(window))
        .cloned()
}

#[test]
#[ignore]
fn x11_toplevel_without_net_wm_pid_is_attributed_and_addressable() {
    if std::env::var_os("DISPLAY").is_none() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some() {
            panic!("the unpublished-pid X11 case requires a native X11 session");
        }
        eprintln!("[x11-unpublished-pid] not a native X11 session; skipping");
        return;
    }
    let cell_id = "linux-xlib-unpublished-pid-left-click-px-background";
    let case = CaseSpec::delivered(
        cell_id,
        "x11-bare-client",
        "xlib",
        "left_click",
        Targeting::Px,
        Delivery::Background,
        Scope::Window,
        DriverRoute::LinuxXSendEvent,
        vec![OracleKind::FixtureState, OracleKind::Protocol],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_named(cell_id).expect("start source-built Linux driver");
        *evidence = recording_evidence(driver.recording_dir());
        let client = BareClient::map();
        let own_pid = std::process::id();
        let window = u64::from(client.window);
        assert!(
            !client.publishes_net_wm_pid(),
            "fixture precondition: the bare client must not publish _NET_WM_PID"
        );

        // Unfiltered enumeration must attribute the window to this process.
        let deadline = Instant::now() + Duration::from_secs(10);
        let record = loop {
            if let Some(record) = listed_record(&mut driver, serde_json::json!({}), window) {
                if record["is_on_screen"].as_bool() == Some(true) {
                    break record;
                }
            }
            assert!(
                Instant::now() < deadline,
                "window {window:#x} ({}) never appeared on screen in list_windows",
                client.title
            );
            std::thread::sleep(Duration::from_millis(200));
        };
        assert_eq!(
            record["pid"].as_u64(),
            Some(u64::from(own_pid)),
            "window without _NET_WM_PID was not attributed to its X-Resource owner: {record}"
        );

        // A pid-filtered listing finds it.
        assert!(
            listed_record(&mut driver, serde_json::json!({"pid": own_pid}), window).is_some(),
            "list_windows {{pid: {own_pid}}} omitted window {window:#x}"
        );

        driver.start_behavior_recording();
        // get_window_state accepts the pid + window_id pair.
        let state = driver.call(
            "get_window_state",
            serde_json::json!({"pid": own_pid, "window_id": window}),
        );
        assert!(
            !state.is_error(),
            "get_window_state refused the attributed window: {}",
            state.text()
        );
        let width = state.structured()["screenshot_width"]
            .as_f64()
            .unwrap_or(0.0);
        let height = state.structured()["screenshot_height"]
            .as_f64()
            .unwrap_or(0.0);
        assert!(
            width > 0.0 && height > 0.0,
            "get_window_state returned no screenshot geometry: {}",
            state.text()
        );

        // A window-scoped background click reaches the window's own connection.
        client.drain_button_presses();
        let clicked = driver.call(
            "click",
            serde_json::json!({
                "pid": own_pid,
                "window_id": window,
                "x": width / 2.0,
                "y": height / 2.0,
                "delivery_mode": "background"
            }),
        );
        assert!(
            !clicked.is_error(),
            "window-scoped click on the attributed window failed: {}",
            clicked.text()
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut presses = 0;
        while presses == 0 && Instant::now() < deadline {
            presses += client.drain_button_presses();
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            presses > 0,
            "the bare client received no ButtonPress from the window-scoped click: {}",
            clicked.text()
        );
        Observation::delivered_with_fixture_state(vec![OracleKind::Protocol])
    });
}
