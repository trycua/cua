// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! macOS window-scoped background pixel clicks on Java AWT/Swing (`libawt_lwawt.dylib`).
//!
//! Java AWT reads delivered `NSEvent` window-local coordinates rather than the
//! global hardware pointer, so PID-routed background `click`, `right_click`,
//! and `double_click` should land at the exact canvas point without moving the
//! hardware pointer, stealing focus, or sending Chromium's off-screen
//! `(-1, -1)` activation primer and 1 ms tap hold.
//!
//! The rows skip with an explicit reason when `javac`/`java` is unavailable
//! unless `CUA_TEST_REQUIRE_JAVA` is set.
//!
//! Run with:
//! `cargo test -p cua-driver-e2e --test java_awt_click_macos_test -- --ignored --nocapture --test-threads=1`

#![cfg(target_os = "macos")]

use std::fs;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::{spawn_in_job, Driver, FixtureJournal, McpDriver};
use serde_json::{json, Value};

const TARGET_CANVAS_X: f64 = 210.0;
const TARGET_CANVAS_Y: f64 = 140.0;

const JAVA_FIXTURE_SOURCE: &str = r#"
import javax.swing.JFrame;
import javax.swing.SwingUtilities;
import java.awt.BorderLayout;
import java.awt.Canvas;
import java.awt.Color;
import java.awt.Dimension;
import java.awt.Graphics;
import java.awt.Point;
import java.awt.event.MouseAdapter;
import java.awt.event.MouseEvent;
import java.awt.event.WindowAdapter;
import java.awt.event.WindowEvent;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

public class JavaAwtCanvasFixture {
    private static final List<String> EVENTS = new ArrayList<>();
    private static final Map<Integer, Long> LAST_PRESS_NS = new HashMap<>();
    private static volatile String journalUrl;
    private static volatile JFrame frameRef;
    private static volatile Canvas canvasRef;
    private static volatile boolean focusStolen = false;

    private static synchronized void postState() {
        try {
            JFrame f = frameRef;
            Canvas c = canvasRef;
            if (f == null || c == null || !f.isShowing() || !c.isShowing()) return;
            Point fLoc = f.getLocationOnScreen();
            Point cLoc = c.getLocationOnScreen();
            StringBuilder ev = new StringBuilder("[");
            for (int i = 0; i < EVENTS.size(); i++) {
                if (i > 0) ev.append(',');
                ev.append(EVENTS.get(i));
            }
            ev.append(']');
            String payload = String.format(
                "{\"ready\":true,\"pid\":%d,\"active\":%s,\"focus_stolen\":%s,"
                + "\"frame\":{\"w\":%d,\"h\":%d},"
                + "\"canvas\":{\"local_x\":%d,\"local_y\":%d,\"w\":%d,\"h\":%d},"
                + "\"events\":%s}",
                ProcessHandle.current().pid(),
                f.isActive(),
                focusStolen,
                f.getWidth(),
                f.getHeight(),
                cLoc.x - fLoc.x,
                cLoc.y - fLoc.y,
                c.getWidth(),
                c.getHeight(),
                ev
            );
            HttpURLConnection conn = (HttpURLConnection) new URL(journalUrl).openConnection();
            conn.setRequestMethod("POST");
            conn.setDoOutput(true);
            conn.setRequestProperty("Content-Type", "application/json");
            byte[] bytes = payload.getBytes(StandardCharsets.UTF_8);
            try (OutputStream os = conn.getOutputStream()) {
                os.write(bytes);
            }
            conn.getResponseCode();
            conn.disconnect();
        } catch (Exception ignored) {
        }
    }

    private static synchronized void recordMouse(String kind, MouseEvent e) {
        long now = System.nanoTime();
        String dur = "null";
        if ("PRESSED".equals(kind)) {
            LAST_PRESS_NS.put(e.getButton(), now);
        } else if ("RELEASED".equals(kind)) {
            Long p = LAST_PRESS_NS.remove(e.getButton());
            if (p != null) {
                dur = String.format(java.util.Locale.ROOT, "%.3f", (now - p) / 1_000_000.0);
            }
        }
        EVENTS.add(String.format(
            "{\"kind\":\"%s\",\"x\":%d,\"y\":%d,\"button\":%d,\"click_count\":%d,\"press_to_release_ms\":%s}",
            kind, e.getX(), e.getY(), e.getButton(), e.getClickCount(), dur
        ));
        postState();
    }

    public static void main(String[] args) throws Exception {
        journalUrl = args[0];
        String title = args[1];
        CountDownLatch ready = new CountDownLatch(1);
        SwingUtilities.invokeLater(() -> {
            JFrame frame = new JFrame(title);
            frame.setDefaultCloseOperation(JFrame.EXIT_ON_CLOSE);
            frame.setLayout(new BorderLayout());
            Canvas canvas = new Canvas() {
                @Override
                public void paint(Graphics g) {
                    g.setColor(new Color(32, 42, 54));
                    g.fillRect(0, 0, getWidth(), getHeight());
                }
            };
            canvas.setPreferredSize(new Dimension(420, 280));
            canvas.addMouseListener(new MouseAdapter() {
                @Override public void mousePressed(MouseEvent e) { recordMouse("PRESSED", e); }
                @Override public void mouseReleased(MouseEvent e) { recordMouse("RELEASED", e); }
                @Override public void mouseClicked(MouseEvent e) { recordMouse("CLICKED", e); }
            });
            frame.add(canvas, BorderLayout.CENTER);
            frame.pack();
            frame.setLocation(140, 140);
            frame.setVisible(true);
            frameRef = frame;
            canvasRef = canvas;
            ready.countDown();
        });
        if (!ready.await(10, TimeUnit.SECONDS)) {
            throw new IllegalStateException("Java AWT frame failed to initialize");
        }
        // Attach focus tracking after initial show/background handoff settles.
        Thread.sleep(600);
        SwingUtilities.invokeLater(() -> {
            frameRef.addWindowListener(new WindowAdapter() {
                @Override public void windowActivated(WindowEvent e) { focusStolen = true; postState(); }
            });
            frameRef.addWindowFocusListener(new WindowAdapter() {
                @Override public void windowGainedFocus(WindowEvent e) { focusStolen = true; postState(); }
            });
            postState();
        });
        Thread.sleep(120_000);
    }
}
"#;

fn java_unavailable() -> Option<String> {
    for bin in ["javac", "java"] {
        match Command::new(bin)
            .arg("-version")
            .stdin(Stdio::null())
            .output()
        {
            Err(error) => {
                let reason = format!("{bin} is unavailable: {error}");
                if std::env::var_os("CUA_TEST_REQUIRE_JAVA").is_some() {
                    panic!("CUA_TEST_REQUIRE_JAVA is set but {reason}");
                }
                return Some(reason);
            }
            Ok(out) if !out.status.success() => {
                let reason = format!("{bin} -version failed with status {}", out.status);
                if std::env::var_os("CUA_TEST_REQUIRE_JAVA").is_some() {
                    panic!("CUA_TEST_REQUIRE_JAVA is set but {reason}");
                }
                return Some(reason);
            }
            Ok(_) => {}
        }
    }
    None
}

struct JavaFixture {
    driver: McpDriver,
    journal: FixtureJournal,
    pid: i64,
    window_id: u64,
    session: String,
    _temp: tempfile::TempDir,
}

fn wait_for(mut predicate: impl FnMut() -> bool, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !predicate() {
        assert!(Instant::now() < deadline, "{message}");
        thread::sleep(Duration::from_millis(50));
    }
}

fn activate_background_keeper(driver: &mut McpDriver, java_pid: i64) {
    let apps = driver.call("list_apps", json!({}));
    let active_pid = apps.structured()["apps"].as_array().and_then(|arr| {
        arr.iter()
            .find(|a| a["active"].as_bool() == Some(true))
            .and_then(|a| a["pid"].as_i64())
    });
    if active_pid == Some(java_pid) {
        let _ = Command::new("osascript")
            .args(["-e", "tell application id \"com.apple.finder\" to activate"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        wait_for(
            || {
                let apps = driver.call("list_apps", json!({}));
                apps.structured()["apps"].as_array().and_then(|arr| {
                    arr.iter()
                        .find(|a| a["active"].as_bool() == Some(true))
                        .and_then(|a| a["pid"].as_i64())
                }) != Some(java_pid)
            },
            "Java fixture remained frontmost before background test",
        );
    }
}

fn launch(label: &str) -> Option<JavaFixture> {
    if let Some(reason) = java_unavailable() {
        eprintln!("[java-awt] SKIP {label}: {reason}");
        return None;
    }
    let temp = tempfile::Builder::new()
        .prefix("cua-java-awt-e2e-")
        .tempdir()
        .expect("create java temp dir");
    let src = temp.path().join("JavaAwtCanvasFixture.java");
    fs::write(&src, JAVA_FIXTURE_SOURCE).expect("write JavaAwtCanvasFixture.java");
    let compile = Command::new("javac")
        .arg("-d")
        .arg(temp.path())
        .arg(&src)
        .stdin(Stdio::null())
        .output()
        .expect("run javac");
    assert!(
        compile.status.success(),
        "javac failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let journal = FixtureJournal::start();
    let title = format!("Cua Java AWT Fixture [{label}]");
    let child = spawn_in_job(
        Command::new("java")
            .arg("-cp")
            .arg(temp.path())
            .arg("JavaAwtCanvasFixture")
            .arg(journal.url())
            .arg(&title)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
    )
    .expect("start Java AWT fixture");

    let mut driver =
        McpDriver::spawn_macos_daemon_proxy_named(label).expect("connect to macOS daemon");
    driver.reaper().push(child);

    // Wait briefly for window, then background Java before attaching focus listeners.
    thread::sleep(Duration::from_millis(300));
    let _ = Command::new("osascript")
        .args(["-e", "tell application id \"com.apple.finder\" to activate"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    wait_for(
        || journal.snapshot()["ready"].as_bool() == Some(true),
        "Java AWT fixture never published readiness",
    );
    let pid = journal.snapshot()["pid"]
        .as_i64()
        .filter(|pid| *pid > 0)
        .expect("fixture journal reports its pid");
    activate_background_keeper(&mut driver, pid);

    let (window_id, _) = driver
        .find_window(pid, &title)
        .expect("find Java AWT fixture window");
    let session = format!("{label}-session");
    let started = driver.call(
        "start_session",
        json!({ "session": session, "capture_scope": "window" }),
    );
    assert!(!started.is_error(), "start_session: {}", started.text());

    Some(JavaFixture {
        driver,
        journal,
        pid,
        window_id,
        session,
        _temp: temp,
    })
}

fn canvas_screenshot_point(fixture: &mut JavaFixture) -> (f64, f64) {
    let state = fixture.driver.call(
        "get_window_state",
        json!({
            "session": fixture.session,
            "pid": fixture.pid,
            "window_id": fixture.window_id,
            "include_accessibility_tree": false,
            "include_screenshot": true,
        }),
    );
    assert!(!state.is_error(), "get_window_state: {}", state.text());
    let shot_w = state.structured()["screenshot_width"]
        .as_f64()
        .expect("screenshot_width");
    let shot_h = state.structured()["screenshot_height"]
        .as_f64()
        .expect("screenshot_height");
    let snap = fixture.journal.snapshot();
    let frame_w = snap["frame"]["w"].as_f64().expect("frame w");
    let frame_h = snap["frame"]["h"].as_f64().expect("frame h");
    let local_x = snap["canvas"]["local_x"].as_f64().expect("canvas local_x");
    let local_y = snap["canvas"]["local_y"].as_f64().expect("canvas local_y");
    let scale_x = shot_w / frame_w;
    let scale_y = shot_h / frame_h;
    (
        (local_x + TARGET_CANVAS_X) * scale_x,
        (local_y + TARGET_CANVAS_Y) * scale_y,
    )
}

fn pointer(fixture: &mut JavaFixture) -> (f64, f64) {
    let pos = fixture.driver.call("get_cursor_position", json!({}));
    assert!(!pos.is_error(), "get_cursor_position: {}", pos.text());
    (
        pos.structured()["x"].as_f64().expect("cursor x"),
        pos.structured()["y"].as_f64().expect("cursor y"),
    )
}

fn events_of(snap: &Value) -> &Vec<Value> {
    snap["events"].as_array().expect("events array")
}

#[test]
#[ignore]
fn java_awt_canvas_background_click_right_click_and_double_click_deliver_without_hijack() {
    let Some(mut fixture) = launch("java-awt-bg-click") else {
        return;
    };
    let (sx, sy) = canvas_screenshot_point(&mut fixture);
    let cursor_before = pointer(&mut fixture);

    // 1. Background left-click: lands at (210, 140), >=15ms hold (not 1ms Chromium tap), 0 focus steal.
    let left = fixture.driver.call(
        "click",
        json!({
            "session": fixture.session,
            "pid": fixture.pid,
            "window_id": fixture.window_id,
            "x": sx,
            "y": sy,
            "button": "left",
            "delivery_mode": "background",
        }),
    );
    assert!(
        !left.is_error(),
        "background left click failed: {}",
        left.raw
    );
    wait_for(
        || {
            let snap = fixture.journal.snapshot();
            events_of(&snap)
                .iter()
                .any(|e| e["kind"] == "CLICKED" && e["button"] == 1 && e["click_count"] == 1)
        },
        &format!(
            "Java AWT left click missing: {}",
            fixture.journal.snapshot()
        ),
    );
    let snap_after_left = fixture.journal.snapshot();
    assert_eq!(snap_after_left["focus_stolen"], false, "{snap_after_left}");
    let left_rel = events_of(&snap_after_left)
        .iter()
        .find(|e| e["kind"] == "RELEASED" && e["button"] == 1)
        .expect("left RELEASED event");
    assert_eq!(left_rel["x"], TARGET_CANVAS_X as i64);
    assert_eq!(left_rel["y"], TARGET_CANVAS_Y as i64);
    let hold_ms = left_rel["press_to_release_ms"]
        .as_f64()
        .expect("press_to_release_ms");
    assert!(
        hold_ms >= 15.0,
        "native background click must hold >=15ms (not 1ms Chromium tap), got {hold_ms}ms"
    );

    // 2. Background right-click: lands at (210, 140) with button=3.
    let prior_len = events_of(&snap_after_left).len();
    let right = fixture.driver.call(
        "right_click",
        json!({
            "session": fixture.session,
            "pid": fixture.pid,
            "window_id": fixture.window_id,
            "x": sx,
            "y": sy,
            "delivery_mode": "background",
        }),
    );
    assert!(
        !right.is_error(),
        "background right click failed: {}",
        right.raw
    );
    wait_for(
        || {
            let snap = fixture.journal.snapshot();
            events_of(&snap)[prior_len.min(events_of(&snap).len())..]
                .iter()
                .any(|e| e["kind"] == "CLICKED" && e["button"] == 3)
        },
        &format!(
            "Java AWT right click missing: {}",
            fixture.journal.snapshot()
        ),
    );
    let snap_after_right = fixture.journal.snapshot();
    assert_eq!(
        snap_after_right["focus_stolen"], false,
        "{snap_after_right}"
    );
    let right_rel = events_of(&snap_after_right)[prior_len..]
        .iter()
        .find(|e| e["kind"] == "RELEASED" && e["button"] == 3)
        .expect("right RELEASED event");
    assert_eq!(right_rel["x"], TARGET_CANVAS_X as i64);
    assert_eq!(right_rel["y"], TARGET_CANVAS_Y as i64);

    // 3. Background double-click: lands at (210, 140) with click_count=2.
    let prior_len = events_of(&snap_after_right).len();
    let dbl = fixture.driver.call(
        "double_click",
        json!({
            "session": fixture.session,
            "pid": fixture.pid,
            "window_id": fixture.window_id,
            "x": sx,
            "y": sy,
            "delivery_mode": "background",
        }),
    );
    assert!(
        !dbl.is_error(),
        "background double click failed: {}",
        dbl.raw
    );
    wait_for(
        || {
            let snap = fixture.journal.snapshot();
            events_of(&snap)[prior_len.min(events_of(&snap).len())..]
                .iter()
                .any(|e| e["kind"] == "CLICKED" && e["button"] == 1 && e["click_count"] == 2)
        },
        &format!(
            "Java AWT double click missing: {}",
            fixture.journal.snapshot()
        ),
    );
    let snap_after_dbl = fixture.journal.snapshot();
    assert_eq!(snap_after_dbl["focus_stolen"], false, "{snap_after_dbl}");
    let dbl_rel = events_of(&snap_after_dbl)[prior_len..]
        .iter()
        .find(|e| e["kind"] == "RELEASED" && e["button"] == 1 && e["click_count"] == 2)
        .expect("double RELEASED event");
    assert_eq!(dbl_rel["x"], TARGET_CANVAS_X as i64);
    assert_eq!(dbl_rel["y"], TARGET_CANVAS_Y as i64);

    let cursor_after = pointer(&mut fixture);
    assert!(
        (cursor_after.0 - cursor_before.0).abs() <= 1.0
            && (cursor_after.1 - cursor_before.1).abs() <= 1.0,
        "hardware cursor moved during background clicks: {cursor_before:?} -> {cursor_after:?}"
    );
}
