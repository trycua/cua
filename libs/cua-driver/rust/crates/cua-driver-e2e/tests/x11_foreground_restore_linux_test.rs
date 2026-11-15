// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Linux/X11 `delivery_mode:"foreground"` activates the target, sends the
//! input, then gives the active window back to the one the user had, as on
//! macOS, Windows and Wayland: `type_text` (CUA-3663), `press_key`, `hotkey`
//! and `click` (CUA-3767), `double_click`, `scroll`, `drag` and `right_click`
//! (CUA-3434). Input that opens a menu leaves the target focused, so the menu
//! stays open, and the result says why.
//!
//! The test process maps two titled toplevels: `prior`, which it activates
//! first, and `target`, whose own connection counts key and button presses
//! and, for the menu cases, maps an override-redirect popup a beat after the
//! first press, as a toolkit menu is. The oracles are those events (fixture
//! state) and the window manager's `_NET_ACTIVE_WINDOW` before and after the
//! call (focus).
//!
//! #[ignore] (needs an X11 display with a window manager). Run:
//!   cargo test -p cua-driver-e2e --test x11_foreground_restore_linux_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "linux")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
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

/// How long after the first press the target maps its menu.
const MENU_DELAY: Duration = Duration::from_millis(100);

fn connect() -> (RustConnection, Window) {
    let (conn, screen_num) =
        RustConnection::connect(None).expect("connect to the X11 display under test");
    let root = conn.setup().roots[screen_num].root;
    (conn, root)
}

fn atom(conn: &RustConnection, name: &[u8]) -> u32 {
    conn.intern_atom(false, name)
        .expect("intern atom")
        .reply()
        .expect("intern atom reply")
        .atom
}

/// A white window owned by `owner` (`_NET_WM_PID`), mapped.
fn map_window(
    conn: &RustConnection,
    root: Window,
    title: &str,
    (x, y, width, height): (i16, i16, u16, u16),
    aux: CreateWindowAux,
    owner: u32,
) -> Window {
    let screen = &conn.setup().roots[0];
    let window = conn.generate_id().expect("allocate X11 window id");
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        x,
        y,
        width,
        height,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &aux.background_pixel(screen.white_pixel),
    )
    .expect("create X11 window");
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )
    .expect("set WM_NAME");
    conn.change_property32(
        PropMode::REPLACE,
        window,
        atom(conn, b"_NET_WM_PID"),
        AtomEnum::CARDINAL,
        &[owner],
    )
    .expect("set _NET_WM_PID");
    // USPosition | USSize: the window manager keeps the asked-for place
    // instead of placing the window itself, so a later test's target is not
    // pushed aside by windows of an earlier one that are still unmapping.
    let mut hints = [0u32; 18];
    hints[0] = 1 | 2;
    hints[1..5].copy_from_slice(&[x as u32, y as u32, width.into(), height.into()]);
    conn.change_property32(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NORMAL_HINTS,
        AtomEnum::WM_SIZE_HINTS,
        &hints,
    )
    .expect("set WM_NORMAL_HINTS");
    conn.map_window(window).expect("map X11 window");
    conn.flush().expect("flush X11 requests");
    window
}

/// The target toplevel on its own connection: a thread counts the key and
/// button presses it receives and, when `popup_owner` is set, maps a popup
/// of that process after the first one: its own is a menu.
struct Target {
    window: Window,
    presses: Arc<AtomicUsize>,
    menu: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Target {
    fn map(popup_owner: Option<u32>) -> Self {
        let (conn, root) = connect();
        let window = map_window(
            &conn,
            root,
            "CuaForegroundRestoreTarget",
            (80, 80, 360, 240),
            CreateWindowAux::new()
                .event_mask(EventMask::KEY_PRESS | EventMask::BUTTON_PRESS | EventMask::EXPOSURE),
            pid(),
        );
        let presses = Arc::new(AtomicUsize::new(0));
        let menu = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (presses, menu, stop) = (presses.clone(), menu.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut menu_due = None;
                while !stop.load(Ordering::SeqCst) {
                    while let Ok(Some(event)) = conn.poll_for_event() {
                        let pressed = match event {
                            Event::KeyPress(press) => press.event == window,
                            Event::ButtonPress(press) => press.event == window,
                            _ => false,
                        };
                        if pressed && presses.fetch_add(1, Ordering::SeqCst) == 0 {
                            menu_due = popup_owner.map(|_| Instant::now() + MENU_DELAY);
                        }
                    }
                    if let Some(owner) =
                        popup_owner.filter(|_| menu_due.is_some_and(|due| Instant::now() >= due))
                    {
                        menu_due = None;
                        let popup = map_window(
                            &conn,
                            root,
                            "CuaForegroundRestoreMenu",
                            (120, 140, 160, 120),
                            CreateWindowAux::new().override_redirect(1),
                            owner,
                        );
                        menu.store(popup as usize, Ordering::SeqCst);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        Self {
            window,
            presses,
            menu,
            stop,
            thread: Some(thread),
        }
    }

    fn wait_presses(&self, at_least: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.presses.load(Ordering::SeqCst) < at_least && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.presses.load(Ordering::SeqCst)
    }

    fn menu_open(&self) -> bool {
        self.menu.load(Ordering::SeqCst) != 0
    }

    fn wait_menu(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.menu_open() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.menu_open()
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Desktop {
    conn: RustConnection,
    root: Window,
    net_active_window: u32,
}

impl Desktop {
    fn connect() -> Self {
        let (conn, root) = connect();
        let net_active_window = atom(&conn, b"_NET_ACTIVE_WINDOW");
        Self {
            conn,
            root,
            net_active_window,
        }
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

    /// The window manager names `window` active and gave it the input focus.
    /// The root property alone can be an earlier test's: its windows are
    /// gone, and a new connection gets the same window ids.
    fn wait_active(&self, window: Window) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let focus = self
                .conn
                .get_input_focus()
                .ok()
                .and_then(|c| c.reply().ok());
            if self.active() == Some(window) && focus.is_some_and(|f| f.focus == window) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Screen pixels of `window`'s point `(x, y)`.
    fn to_screen(&self, window: Window, x: i16, y: i16) -> (i16, i16) {
        let reply = self
            .conn
            .translate_coordinates(window, self.root, x, y)
            .expect("translate coordinates")
            .reply()
            .expect("translate coordinates reply");
        (reply.dst_x, reply.dst_y)
    }
}

/// What the call must leave behind.
#[derive(Clone, Copy, PartialEq)]
enum Expect {
    /// The prior window is active again.
    Restored,
    /// The menu the input opened is up and the target kept the focus.
    MenuKeptFocus,
    /// Another process's popup mapped after the input, and the prior window
    /// is active again all the same.
    RestoredPastForeignPopup,
}

impl Expect {
    /// The pid the popup that follows the first press says it belongs to.
    fn popup_owner(self) -> Option<u32> {
        match self {
            Self::Restored => None,
            Self::MenuKeptFocus => Some(pid()),
            // init: never the target's process.
            Self::RestoredPastForeignPopup => Some(1),
        }
    }
}

fn native_x11_session() -> bool {
    if std::env::var_os("DISPLAY").is_some() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return true;
    }
    if std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some() {
        panic!("the X11 foreground restore case requires a native X11 session");
    }
    eprintln!("[x11-foreground-restore] not a native X11 session; skipping");
    false
}

/// Run `tool` in the foreground against a fresh target while `prior` is
/// active, and check the target got `presses` presses and where the focus
/// ended up. `args` receives the target window and its screen origin.
fn foreground_case(
    action: &str,
    targeting: Targeting,
    tool: &str,
    presses: usize,
    expect: Expect,
    args: impl FnOnce(Window, (i16, i16)) -> serde_json::Value,
) {
    if !native_x11_session() {
        return;
    }
    let menu = match expect {
        Expect::Restored => "",
        Expect::MenuKeptFocus => "-menu",
        Expect::RestoredPastForeignPopup => "-foreign-popup",
    };
    let targeting_label = match targeting {
        Targeting::Px => "px",
        _ => "ax",
    };
    let cell_id = format!(
        "linux-xlib-foreground-restore-{}{menu}-{targeting_label}-foreground",
        tool.replace('_', "-")
    );
    let case = CaseSpec::delivered(
        cell_id.clone(),
        "x11-bare-client",
        "xlib",
        action,
        targeting,
        Delivery::Foreground,
        Scope::Window,
        DriverRoute::LinuxXTest,
        vec![OracleKind::FixtureState, OracleKind::Focus],
    );
    execute_case(case, |evidence| {
        // With the post-action window watch off, the restore runs right after
        // the input, before the menu maps: only a restore that waits for a
        // late menu keeps it open.
        let env: &[(&str, &str)] = if expect.popup_owner().is_some() {
            &[("CUA_DRIVER_WINDOW_CHANGE_TIMEOUT_MS", "0")]
        } else {
            &[]
        };
        let mut driver = McpDriver::spawn_named_with_env(&cell_id, env)
            .expect("start source-built Linux driver");
        *evidence = recording_evidence(driver.recording_dir());
        let desktop = Desktop::connect();
        let target = Target::map(expect.popup_owner());
        let prior = map_window(
            &desktop.conn,
            desktop.root,
            "CuaForegroundRestorePrior",
            (520, 80, 360, 240),
            CreateWindowAux::new().event_mask(EventMask::EXPOSURE),
            pid(),
        );
        desktop.activate(prior);
        assert!(
            desktop.wait_active(prior),
            "fixture precondition: the prior window never became active"
        );
        let origin = desktop.to_screen(target.window, 0, 0);

        driver.start_behavior_recording();
        let response = driver.call(tool, args(target.window, origin));
        assert!(
            !response.is_error(),
            "foreground {tool} failed: {}",
            response.text()
        );
        let received = target.wait_presses(presses);
        assert!(
            received >= presses,
            "the target received {received} of {presses} presses: {}",
            response.text()
        );
        match expect {
            Expect::Restored | Expect::RestoredPastForeignPopup => {
                if expect == Expect::RestoredPastForeignPopup {
                    assert!(target.wait_menu(), "fixture: the popup never mapped");
                }
                assert_eq!(
                    desktop.active(),
                    Some(prior),
                    "foreground {tool} left the active window on the target instead of \
                     the prior window: {}",
                    response.text()
                );
            }
            Expect::MenuKeptFocus => {
                assert!(target.menu_open(), "fixture: the menu never mapped");
                assert_eq!(
                    desktop.active(),
                    Some(target.window),
                    "foreground {tool} gave the focus back while its menu was open, \
                     which closes a real menu: {}",
                    response.text()
                );
                assert!(
                    response
                        .text()
                        .contains("left focused because a menu is open"),
                    "foreground {tool} did not say why the target kept the focus: {}",
                    response.text()
                );
            }
        }
        Observation::delivered_with_fixture_state(vec![OracleKind::Focus])
    });
}

fn pid() -> u32 {
    std::process::id()
}

#[test]
#[ignore]
fn x11_foreground_type_text_restores_the_prior_active_window() {
    foreground_case(
        "type_text",
        Targeting::Px,
        "type_text",
        3,
        Expect::Restored,
        |target, _| {
            serde_json::json!({
                "pid": pid(), "window_id": u64::from(target), "text": "abc",
                "delivery_mode": "foreground"
            })
        },
    );
}

#[test]
#[ignore]
fn x11_foreground_press_key_restores_the_prior_active_window() {
    foreground_case(
        "press_key",
        Targeting::Px,
        "press_key",
        1,
        Expect::Restored,
        |target, _| {
            serde_json::json!({
                "pid": pid(), "window_id": u64::from(target), "key": "a",
                "delivery_mode": "foreground"
            })
        },
    );
}

#[test]
#[ignore]
fn x11_foreground_hotkey_restores_the_prior_active_window() {
    foreground_case(
        "hotkey",
        Targeting::Px,
        "hotkey",
        2,
        Expect::Restored,
        |target, _| {
            serde_json::json!({
                "pid": pid(), "window_id": u64::from(target), "keys": ["ctrl", "a"],
                "delivery_mode": "foreground"
            })
        },
    );
}

fn click_args(target: Window, (x, y): (i16, i16)) -> serde_json::Value {
    serde_json::json!({
        "pid": pid(), "window_id": u64::from(target),
        "x": x + 100, "y": y + 100, "coordinate_frame": "desktop",
        "delivery_mode": "foreground"
    })
}

#[test]
#[ignore]
fn x11_foreground_click_restores_the_prior_active_window() {
    foreground_case(
        "left_click",
        Targeting::Px,
        "click",
        1,
        Expect::Restored,
        click_args,
    );
}

#[test]
#[ignore]
fn x11_foreground_click_that_opens_a_menu_keeps_the_target_focused() {
    foreground_case(
        "left_click",
        Targeting::Px,
        "click",
        1,
        Expect::MenuKeptFocus,
        click_args,
    );
}

#[test]
#[ignore]
fn x11_foreground_type_text_that_opens_a_menu_succeeds_and_keeps_it_open() {
    foreground_case(
        "type_text",
        Targeting::Px,
        "type_text",
        1,
        Expect::MenuKeptFocus,
        |target, _| {
            serde_json::json!({
                "pid": pid(), "window_id": u64::from(target), "text": "m",
                "delivery_mode": "foreground"
            })
        },
    );
}

#[test]
#[ignore]
fn x11_foreground_click_restores_past_another_process_popup() {
    foreground_case(
        "left_click",
        Targeting::Px,
        "click",
        1,
        Expect::RestoredPastForeignPopup,
        click_args,
    );
}

#[test]
#[ignore]
fn x11_foreground_double_click_restores_the_prior_active_window() {
    foreground_case(
        "double_click",
        Targeting::Px,
        "double_click",
        2,
        Expect::Restored,
        click_args,
    );
}

#[test]
#[ignore]
fn x11_foreground_right_click_restores_the_prior_active_window() {
    foreground_case(
        "right_click",
        Targeting::Px,
        "right_click",
        1,
        Expect::Restored,
        click_args,
    );
}

#[test]
#[ignore]
fn x11_foreground_right_click_that_opens_a_menu_keeps_the_target_focused() {
    foreground_case(
        "right_click",
        Targeting::Px,
        "right_click",
        1,
        Expect::MenuKeptFocus,
        click_args,
    );
}

#[test]
#[ignore]
fn x11_foreground_scroll_restores_the_prior_active_window() {
    // Each wheel detent is a button 4/5 press on the target.
    foreground_case(
        "scroll",
        Targeting::Px,
        "scroll",
        2,
        Expect::Restored,
        |target, (x, y)| {
            serde_json::json!({
                "pid": pid(), "window_id": u64::from(target),
                "x": x + 100, "y": y + 100, "coordinate_frame": "desktop",
                "direction": "down", "amount": 2,
                "delivery_mode": "foreground"
            })
        },
    );
}

#[test]
#[ignore]
fn x11_foreground_drag_restores_the_prior_active_window() {
    foreground_case(
        "drag",
        Targeting::Px,
        "drag",
        1,
        Expect::Restored,
        |target, (x, y)| {
            serde_json::json!({
                "pid": pid(), "window_id": u64::from(target),
                "from_x": x + 60, "from_y": y + 60, "to_x": x + 200, "to_y": y + 160,
                "coordinate_frame": "desktop",
                "delivery_mode": "foreground"
            })
        },
    );
}
