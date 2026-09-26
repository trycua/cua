//! macOS window-scoped pixel clicks on a toolkit that reads the hardware
//! pointer (D-MAC-6).
//!
//! Tk's macOS backend derives a mouse event's location from the global
//! pointer, not from the delivered CGEvent. The repository's Tk
//! `visual-only-canvas` fixture therefore logged `ignored click outside cards`
//! for window-scoped clicks even at the correct screenshot point, because the
//! PID-routed event never moved the pointer. These rows prove the contract:
//!
//! * `delivery_mode:"foreground"` activates the exact window, moves the
//!   hardware pointer to the mapped point, and posts through the HID tap, so
//!   the fixture's own loopback oracle records exactly one `send` selection
//!   and the pointer is left at the target (as on Windows and X11).
//! * `delivery_mode:"background"` returns the structured
//!   `background_unavailable` refusal instead of an unverifiable success, and
//!   the oracle is unchanged.
//!
//! The rows skip with an explicit reason when `python3` lacks Tk support
//! (GitHub-hosted runners may not ship it) unless `CUA_TEST_REQUIRE_TK` is
//! set.
//!
//! Run with:
//! `cargo test -p cua-driver-e2e --test tk_pointer_click_macos_test -- --ignored --nocapture --test-threads=1`

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::{spawn_in_job, Driver, FixtureJournal, McpDriver};
use serde_json::{json, Value};

/// Canvas size and the Send card center from the fixture's `CARDS` table.
const CANVAS_WIDTH: f64 = 760.0;
const CANVAS_HEIGHT: f64 = 460.0;
const SEND_CENTER: (f64, f64) = (394.0, 221.0);
const SEND_BOUNDS: (f64, f64, f64, f64) = (292.0, 132.0, 496.0, 310.0);

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/apps/cross-platform/visual-only-canvas/main.py")
}

/// Return a skip reason when this host cannot run the Tk fixture.
fn tk_unavailable() -> Option<String> {
    let probe = Command::new("python3")
        .args(["-c", "import tkinter; print(tkinter.TkVersion)"])
        .stdin(Stdio::null())
        .output();
    let reason = match probe {
        Err(error) => format!("python3 is unavailable: {error}"),
        Ok(output) if !output.status.success() => format!(
            "python3 has no importable tkinter: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Ok(_) => return None,
    };
    if std::env::var_os("CUA_TEST_REQUIRE_TK").is_some() {
        panic!("CUA_TEST_REQUIRE_TK is set but {reason}");
    }
    Some(reason)
}

struct TkFixture {
    driver: McpDriver,
    journal: FixtureJournal,
    pid: i64,
    window_id: u64,
    session: String,
}

fn launch(label: &str) -> Option<TkFixture> {
    if let Some(reason) = tk_unavailable() {
        eprintln!("[tk-pointer] SKIP {label}: {reason}");
        return None;
    }
    let journal = FixtureJournal::start();
    let title = format!("Cua Tk Pointer Fixture [{label}]");
    let child = spawn_in_job(
        Command::new("python3")
            .arg(fixture_path())
            .args(["--journal-url", journal.url(), "--title", &title])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
    )
    .expect("start the Tk visual-only-canvas fixture");
    let mut driver =
        McpDriver::spawn_macos_daemon_proxy_named(label).expect("connect to the macOS daemon");
    driver.reaper().push(child);
    wait_for(
        || journal.snapshot()["ready"].as_bool() == Some(true),
        "Tk fixture never published readiness",
    );
    let pid = journal.snapshot()["pid"]
        .as_i64()
        .filter(|pid| *pid > 0)
        .expect("fixture journal reports its pid");
    let (window_id, _) = driver
        .find_window(pid, &title)
        .expect("find the exact Tk fixture window");
    let session = format!("{label}-session");
    let started = driver.call(
        "start_session",
        json!({"session": session, "capture_scope": "window"}),
    );
    assert!(!started.is_error(), "start_session: {}", started.text());
    Some(TkFixture {
        driver,
        journal,
        pid,
        window_id,
        session,
    })
}

fn wait_for(mut predicate: impl FnMut() -> bool, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "{message}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Window-local screenshot pixel for the Send card center. The capture spans
/// the full window: the canvas fills its width and sits under the title bar.
fn send_screenshot_point(fixture: &mut TkFixture) -> (f64, f64) {
    let state = fixture.driver.call(
        "get_window_state",
        json!({
            "session": fixture.session,
            "pid": fixture.pid,
            "window_id": fixture.window_id,
        }),
    );
    assert!(!state.is_error(), "get_window_state: {}", state.text());
    let width = state.structured()["screenshot_width"]
        .as_f64()
        .expect("screenshot width");
    let height = state.structured()["screenshot_height"]
        .as_f64()
        .expect("screenshot height");
    let scale = width / CANVAS_WIDTH;
    let title_bar = height - CANVAS_HEIGHT * scale;
    assert!(
        scale > 0.0 && (0.0..=80.0 * scale).contains(&title_bar),
        "unexpected Tk window capture geometry {width}x{height}"
    );
    (SEND_CENTER.0 * scale, title_bar + SEND_CENTER.1 * scale)
}

fn window_bounds(fixture: &mut TkFixture) -> Value {
    let windows = fixture
        .driver
        .call("list_windows", json!({ "pid": fixture.pid }));
    windows.structured()["windows"]
        .as_array()
        .and_then(|windows| {
            windows
                .iter()
                .find(|window| window["window_id"].as_u64() == Some(fixture.window_id))
        })
        .map(|window| window["bounds"].clone())
        .expect("fixture window bounds")
}

fn pointer(fixture: &mut TkFixture) -> (f64, f64) {
    let position = fixture.driver.call("get_cursor_position", json!({}));
    assert!(
        !position.is_error(),
        "get_cursor_position: {}",
        position.text()
    );
    (
        position.structured()["x"].as_f64().expect("pointer x"),
        position.structured()["y"].as_f64().expect("pointer y"),
    )
}

#[test]
#[ignore]
fn tk_window_pixel_click_foreground_moves_pointer_and_reaches_target() {
    let Some(mut fixture) = launch("tk-pointer-foreground") else {
        return;
    };
    let (x, y) = send_screenshot_point(&mut fixture);
    let click = fixture.driver.call(
        "click",
        json!({
            "pid": fixture.pid,
            "window_id": fixture.window_id,
            "x": x,
            "y": y,
            "delivery_mode": "foreground",
        }),
    );
    assert!(!click.is_error(), "foreground click failed: {}", click.raw);
    assert_eq!(click.action_route(), Some("global_input"), "{}", click.raw);
    assert_eq!(
        click.action_delivery_mode(),
        Some("foreground"),
        "{}",
        click.raw
    );
    assert!(
        click.text().contains("hardware pointer moved"),
        "result must say the pointer moved: {}",
        click.text()
    );
    wait_for(
        || {
            let state = fixture.journal.snapshot();
            state["selected"] == "send" && state["action_count"] == 1
        },
        &format!(
            "Tk oracle did not record exactly one Send selection: {}",
            fixture.journal.snapshot()
        ),
    );

    // The foreground contract leaves the hardware pointer at the target, like
    // Windows SendInput and X11 XTest. Allow a small title-bar tolerance.
    let bounds = window_bounds(&mut fixture);
    let (px, py) = pointer(&mut fixture);
    let left = bounds["x"].as_f64().unwrap();
    let bottom = bounds["y"].as_f64().unwrap() + bounds["height"].as_f64().unwrap();
    let canvas_top = bottom - CANVAS_HEIGHT;
    let inside_send = px >= left + SEND_BOUNDS.0
        && px <= left + SEND_BOUNDS.2
        && py >= canvas_top + SEND_BOUNDS.1 - 4.0
        && py <= canvas_top + SEND_BOUNDS.3 + 4.0;
    assert!(
        inside_send,
        "hardware pointer ({px},{py}) is not over the Send card; window bounds {bounds}"
    );
}

#[test]
#[ignore]
fn tk_window_pixel_click_background_is_refused_without_delivery() {
    let Some(mut fixture) = launch("tk-pointer-background") else {
        return;
    };
    let (x, y) = send_screenshot_point(&mut fixture);
    let click = fixture.driver.call(
        "click",
        json!({
            "pid": fixture.pid,
            "window_id": fixture.window_id,
            "x": x,
            "y": y,
            "delivery_mode": "background",
        }),
    );
    assert!(
        click.is_error(),
        "background Tk click must refuse: {}",
        click.raw
    );
    let structured = click.structured();
    assert_eq!(
        structured["code"], "background_unavailable",
        "{}",
        click.raw
    );
    assert_eq!(
        structured["reason"], "pointer_reading_toolkit",
        "{}",
        click.raw
    );
    assert_eq!(structured["toolkit"], "tk", "{}", click.raw);
    assert_eq!(
        structured["escalation"]["recommended"], "foreground",
        "{}",
        click.raw
    );
    thread::sleep(Duration::from_millis(500));
    let state = fixture.journal.snapshot();
    assert!(
        state["selected"].is_null() && state["action_count"] == 0,
        "a refused background click must not reach the fixture: {state}"
    );
}
