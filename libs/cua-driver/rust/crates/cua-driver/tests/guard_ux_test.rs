//! UX-guard integration tests for Windows.
//!
//! These mirror the Python e2e tests in libs/cua-driver/tests/e2e/:
//!   - test_background_focus.py
//!   - test_click_opens_new_window.py
//!   - test_launch_app_visible.py
//!   - test_background_menu_shortcut.py
//!
//! Invariant under test (the "UX guard"):
//!   The agent must be able to click, type, and launch apps in background
//!   windows WITHOUT stealing focus from the user's foreground window.
//!
//! Background target: the repo-local Electron harness staged at
//!   test-apps/harness-electron/CuaTestHarness.Electron.exe. Notepad is used
//!   only as a secondary fallback check.
//!
//! Background-action tests launch the target first, then foreground
//! focus-monitor-win (the "user's foreground window") before measuring focus
//! loss. The launch_app test starts the monitor first because launch behavior is
//! the action under test.
//!
//! Run in sandbox via:
//!   .\sandbox\run-tests-in-sandbox.ps1 ux_guard

#![cfg(target_os = "windows")]

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::{ax, driver_binary, spawn_in_job, workspace_root, Driver, McpDriver};

// ── focus-monitor + test-app fixtures ────────────────────────────────────────

fn gui_required() -> bool {
    std::env::var("CUA_REQUIRE_GUI")
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn skip_desktop(context: &str, reason: String) -> bool {
    let msg = format!("{context}: {reason}; skipping GUI UX guard");
    if gui_required() {
        panic!("{msg}");
    }
    eprintln!("{msg}");
    false
}

fn require_seedable_desktop(context: &str) -> bool {
    let state = platform_windows::diagnostics::desktop_state();
    if state.session_id == Some(0) {
        return skip_desktop(
            context,
            format!(
                "running in Windows Session 0 ({}) - re-run from RDP/console/scheduled task in user session",
                state.summary()
            ),
        );
    }
    if !state.has_process_window_station {
        return skip_desktop(
            context,
            format!("no attached process window station ({})", state.summary()),
        );
    }
    if !state.input_desktop_is_default() {
        return skip_desktop(
            context,
            format!(
                "input desktop is not the user Default desktop ({})",
                state.summary()
            ),
        );
    }
    true
}

fn require_focus_monitor_foreground(context: &str, expected_hwnd: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let state = platform_windows::diagnostics::desktop_state();
        if state.foreground_hwnd == Some(expected_hwnd as usize) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return skip_desktop(
                context,
                format!(
                    "focus monitor did not become foreground (expected HWND 0x{expected_hwnd:x}; {})",
                    state.summary()
                ),
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn focus_monitor_path() -> PathBuf {
    workspace_root().join("target/debug/focus-monitor-win.exe")
}

fn test_app_path() -> PathBuf {
    // In sandbox, sandbox-runner.ps1 copies the exe to %TEMP% to avoid the
    // ShellExecuteW zone-security dialog that blocks on mapped-folder exes.
    if let Ok(p) = std::env::var("HARNESS_ELECTRON_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return pb;
        }
    }
    workspace_root().join("test-apps/harness-electron/CuaTestHarness.Electron.exe")
}

/// Launch the Electron harness in the background (tied to the driver's reaper)
/// and return its pid. Returns None if the binary doesn't exist or the app
/// fails to start.
fn launch_test_app(driver: &mut McpDriver) -> Option<u32> {
    let exe = test_app_path();
    if !exe.exists() {
        eprintln!("CuaTestHarness.Electron not found at {exe:?} - skipping");
        return None;
    }
    let child = spawn_in_job(
        Command::new(&exe)
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
    .ok()?;
    let pid = child.id();
    driver.reaper().push(child);
    // Cold-start in sandbox can take a few seconds.
    std::thread::sleep(Duration::from_secs(3));
    Some(pid)
}

fn launch_driver_and_test_app() -> Option<(McpDriver, u32, u64)> {
    let Some(mut driver) = McpDriver::spawn() else {
        return None;
    };

    let Some(app_pid) = launch_test_app(&mut driver) else {
        eprintln!("test app not available — skipping");
        return None;
    };
    let Some(app_wid) = find_window_for_pid(&mut driver, app_pid as i64) else {
        eprintln!("test app window not found — skipping");
        return None;
    };

    Some((driver, app_pid, app_wid))
}

fn kill_process_tree_by_image(image: &str) {
    Command::new("taskkill")
        .args(["/F", "/T", "/IM", image])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok();
}

struct KillProcessTreeOnDrop(&'static str);

impl Drop for KillProcessTreeOnDrop {
    fn drop(&mut self) {
        kill_process_tree_by_image(self.0);
    }
}

fn loss_file() -> PathBuf {
    std::env::temp_dir().join("focus_monitor_losses.txt")
}
fn key_loss_file() -> PathBuf {
    std::env::temp_dir().join("focus_monitor_key_losses.txt")
}

fn read_losses(path: &std::path::Path) -> u32 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn focus_pid_file() -> PathBuf {
    std::env::temp_dir().join("focus_monitor_pid.txt")
}
fn focus_hwnd_file() -> PathBuf {
    std::env::temp_dir().join("focus_monitor_hwnd.txt")
}

/// Launch focus-monitor-win and return (process, hwnd, pid).
/// Reads FOCUS_PID and FOCUS_HWND from temp files written by the monitor
/// (avoids blocking on the stdout pipe if the sandbox redirects I/O).
fn launch_focus_monitor() -> Option<(Child, u64, u32)> {
    if !require_seedable_desktop("focus-monitor-win") {
        return None;
    }

    let exe = focus_monitor_path();
    if !exe.exists() {
        eprintln!("focus-monitor-win.exe not built at {exe:?} — skipping");
        return None;
    }
    // Reset all sentinel files so stale values are not mistaken for new ones.
    let _ = std::fs::write(loss_file(), "0");
    let _ = std::fs::write(key_loss_file(), "0");
    let _ = std::fs::remove_file(focus_pid_file());
    let _ = std::fs::remove_file(focus_hwnd_file());

    let mut child = spawn_in_job(
        Command::new(&exe)
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
    .expect("spawn focus-monitor-win");

    // Poll temp files until both PID and HWND are written (max 15s).
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let (mut pid_val, mut hwnd_val) = (0u32, 0u64);
    loop {
        if std::time::Instant::now() > deadline {
            panic!("focus-monitor-win did not write PID/HWND temp files within 15s");
        }
        if pid_val == 0 {
            pid_val = std::fs::read_to_string(focus_pid_file())
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
        }
        if hwnd_val == 0 {
            hwnd_val = std::fs::read_to_string(focus_hwnd_file())
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
        }
        if pid_val != 0 && hwnd_val != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !require_focus_monitor_foreground("focus-monitor-win", hwnd_val) {
        child.kill().ok();
        return None;
    }
    Some((child, hwnd_val, pid_val))
}

/// Find the first on-screen window belonging to the given pid.
fn find_window_for_pid(driver: &mut McpDriver, pid: i64) -> Option<u64> {
    let resp = driver.call(
        "list_windows",
        serde_json::json!({"pid": pid, "on_screen_only": true}),
    );
    resp.structured()["windows"]
        .as_array()?
        .iter()
        .find_map(|w| w["window_id"].as_u64())
}

fn window_ids(driver: &mut McpDriver) -> HashSet<u64> {
    let resp = driver.call("list_windows", serde_json::json!({}));
    resp.structured()["windows"]
        .as_array()
        .map(|a| a.iter().filter_map(|w| w["window_id"].as_u64()).collect())
        .unwrap_or_default()
}

fn wait_for_new_window(driver: &mut McpDriver, before: &HashSet<u64>) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let after = window_ids(driver);
        if after.iter().any(|id| !before.contains(id)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

// ── UX guard assertion ────────────────────────────────────────────────────────

/// Assert act_losses stayed at `max_allowed` (usually 0) since `before`.
fn assert_ux_guard(before: u32, max_allowed: u32, context: &str) {
    let after = read_losses(&loss_file());
    let delta = after.saturating_sub(before);
    assert!(
        delta <= max_allowed,
        "UX guard violated: act_losses went from {before} to {after} \
         (delta={delta}, max_allowed={max_allowed}) during: {context}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 1: background click + type do not steal focus
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_background_click_and_type_no_focus_steal() {
    //! Equivalent of macOS test_background_focus.py.
    //!
    //! 1. Launch the Electron harness.
    //! 2. Foreground FocusMonitorWin (simulates the user's active window).
    //! 3. Click inside the app and type text via cua-driver.
    //! 4. Assert act_losses on FocusMonitorWin stayed at 0.

    if !driver_binary().exists() {
        eprintln!("Binary not found — skipping");
        return;
    }

    let Some((mut driver, app_pid, app_wid)) = launch_driver_and_test_app() else {
        return;
    };

    let Some((mut fm_proc, _fm_hwnd, _fm_pid)) = launch_focus_monitor() else {
        return;
    };
    let losses_before = read_losses(&loss_file());

    // Click inside the app (background, via PostMessage).
    let r = driver.call(
        "click",
        serde_json::json!({"pid": app_pid, "window_id": app_wid, "x": 200.0, "y": 200.0}),
    );
    assert!(
        r.raw["error"].is_null(),
        "Protocol error from click: {:?}",
        r.raw
    );

    // Type text into the app (background, via PostMessage).
    let r = driver.call(
        "type_text",
        serde_json::json!({"pid": app_pid, "window_id": app_wid, "text": "ux-guard-test"}),
    );
    assert!(
        r.raw["error"].is_null(),
        "Protocol error from type_text: {:?}",
        r.raw
    );
    assert_eq!(
        r.verified(),
        Some(false),
        "background type_text without an element read-back must not report confirmed success: {}",
        r.text()
    );
    assert_ne!(
        r.structured()["verify"].as_str(),
        Some("confirmed"),
        "background type_text reported a confirmed read-back on an unreadable path: {}",
        r.text()
    );

    // Plain background key dispatch is likewise an unverified PostMessage send,
    // not a confirmed keypress.
    let r = driver.call(
        "press_key",
        serde_json::json!({"pid": app_pid, "window_id": app_wid, "key": "F24"}),
    );
    assert!(
        r.raw["error"].is_null(),
        "Protocol error from press_key: {:?}",
        r.raw
    );
    assert_eq!(
        r.verified(),
        Some(false),
        "background press_key must not report confirmed success: {}",
        r.text()
    );

    // ux_guard: FocusMonitorWin must not have lost activation.
    assert_ux_guard(
        losses_before,
        0,
        "background click + type_text into CuaTestHarness.Electron",
    );

    fm_proc.kill().ok();
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: launch_app minimized mode does not steal focus
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_launch_app_minimized_no_focus_steal() {
    //! Equivalent of macOS test_launch_app_visible.py.
    //!
    //! launch_app with start_minimized=true is the strict Windows background
    //! launch mode: the app starts without displacing FocusMonitorWin.

    if !driver_binary().exists() {
        return;
    }

    let exe = test_app_path();
    if !exe.exists() {
        eprintln!("test app not available — skipping");
        return;
    }
    let _cleanup = KillProcessTreeOnDrop("CuaTestHarness.Electron.exe");

    let Some((mut fm_proc, _fm_hwnd, _fm_pid)) = launch_focus_monitor() else {
        return;
    };
    let losses_before = read_losses(&loss_file());

    let Some(mut driver) = McpDriver::spawn() else {
        fm_proc.kill().ok();
        return;
    };

    // Launch the test app via cua-driver launch_app in strict background mode.
    let path_str = exe.to_string_lossy().into_owned();
    let r = driver.call(
        "launch_app",
        serde_json::json!({"path": path_str, "start_minimized": true}),
    );
    if r.is_error() {
        eprintln!("launch_app failed — skipping: {:?}", r.raw);
        fm_proc.kill().ok();
        return;
    }

    // Wait for the app window to appear (Electron startup ~2-3s).
    let mut app_pid: Option<i64> = None;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(500));
        let r2 = driver.call("list_apps", serde_json::json!({}));
        if let Some(procs) = r2.structured()["processes"].as_array() {
            if let Some(p) = procs.iter().find(|p| {
                p["name"]
                    .as_str()
                    .map(|n| {
                        let n = n.to_lowercase();
                        n.contains("cuatestharness.electron") || n.contains("electron")
                    })
                    .unwrap_or(false)
            }) {
                app_pid = p["pid"].as_i64();
                break;
            }
        }
    }
    if app_pid.is_none() {
        eprintln!("CuaTestHarness.Electron not found in process list after launch_app - skipping");
        fm_proc.kill().ok();
        return;
    }

    // ux_guard: FocusMonitorWin must not have lost activation.
    assert_ux_guard(
        losses_before,
        0,
        "launch_app CuaTestHarness.Electron with start_minimized=true",
    );

    // Kill the launched app by exe name.
    kill_process_tree_by_image("CuaTestHarness.Electron.exe");
    fm_proc.kill().ok();
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3: background hotkey does not steal focus
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_background_hotkey_no_focus_steal() {
    //! Equivalent of macOS test_background_menu_shortcut.py.
    //!
    //! Send Ctrl+A to a background Electron harness window.
    //! FocusMonitorWin must never lose activation.

    if !driver_binary().exists() {
        return;
    }

    let Some((mut driver, app_pid, app_wid)) = launch_driver_and_test_app() else {
        return;
    };

    let Some((mut fm_proc, _fm_hwnd, _fm_pid)) = launch_focus_monitor() else {
        return;
    };
    let losses_before = read_losses(&loss_file());

    // Send Ctrl+A hotkey to background app (PostMessage, no focus steal).
    let r = driver.call(
        "hotkey",
        serde_json::json!({"pid": app_pid, "window_id": app_wid, "keys": ["ctrl", "a"]}),
    );
    assert!(
        r.raw["error"].is_null(),
        "Protocol error from hotkey: {:?}",
        r.raw
    );

    // ux_guard: FocusMonitorWin must not have lost activation.
    assert_ux_guard(
        losses_before,
        0,
        "background hotkey ctrl+a to CuaTestHarness.Electron",
    );

    fm_proc.kill().ok();
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4: background click that opens a new window (e.g. File→New dialog)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_background_click_opens_new_window_focus_preserved() {
    //! Equivalent of macOS test_click_opens_new_window.py.
    //!
    //! 1. FocusMonitorWin is foreground.
    //! 2. Click the CuaTestHarness.Electron child-window button.
    //! 3. FocusMonitorWin must remain active throughout (UX guard).
    //!
    //! Verifies that a background click can cause a child window to appear
    //! without activating either the original target or the new window.

    if !driver_binary().exists() {
        return;
    }

    let Some((mut driver, app_pid, app_wid)) = launch_driver_and_test_app() else {
        return;
    };
    let snap = driver.call(
        "get_window_state",
        serde_json::json!({"pid": app_pid, "window_id": app_wid, "capture_mode": "ax"}),
    );
    let Some(open_idx) = ax::element_index_containing(snap.text(), "Open child window") else {
        eprintln!("child-window button not found in test app — skipping");
        return;
    };

    let Some((mut fm_proc, _fm_hwnd, _fm_pid)) = launch_focus_monitor() else {
        return;
    };
    let windows_before = window_ids(&mut driver);
    let losses_before = read_losses(&loss_file());

    // Click the explicit child-window button.
    let r = driver.call(
        "click",
        serde_json::json!({"pid": app_pid, "window_id": app_wid, "element_index": open_idx}),
    );
    assert!(
        r.raw["error"].is_null(),
        "Protocol error from click: {:?}",
        r.raw
    );

    assert!(
        wait_for_new_window(&mut driver, &windows_before),
        "background click did not open a new harness window"
    );

    // ux_guard: FocusMonitorWin must not have lost activation.
    assert_ux_guard(
        losses_before,
        0,
        "background click in CuaTestHarness.Electron (may open new window)",
    );

    // Verify FocusMonitorWin is still alive.
    assert!(
        fm_proc.try_wait().expect("try_wait").is_none(),
        "FocusMonitorWin crashed during the test"
    );

    fm_proc.kill().ok();
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5: screenshot of background window doesn't steal focus
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_background_screenshot_no_focus_steal() {
    //! PrintWindow captures a background window without activating it.

    if !driver_binary().exists() {
        return;
    }

    let Some((mut driver, _app_pid, app_wid)) = launch_driver_and_test_app() else {
        return;
    };

    let Some((mut fm_proc, _fm_hwnd, _fm_pid)) = launch_focus_monitor() else {
        return;
    };
    let losses_before = read_losses(&loss_file());

    // Screenshot via PrintWindow — must not activate the window.
    let r = driver.call("screenshot", serde_json::json!({"window_id": app_wid}));
    assert!(
        r.raw["error"].is_null(),
        "Protocol error from screenshot: {:?}",
        r.raw
    );

    // ux_guard
    assert_ux_guard(
        losses_before,
        0,
        "screenshot of background CuaTestHarness.Electron",
    );

    fm_proc.kill().ok();
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6: agent cursor is visually present on screen after move_cursor
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_agent_cursor_visible_on_screen() {
    //! Computer-vision check: after move_cursor the overlay must be visible.
    //!
    //! Steps:
    //!   1. Enable the agent cursor and move it to a known screen position.
    //!   2. Wait for the glide animation to settle (default 750ms).
    //!   3. Capture the screen at the cursor position using screenshot_display_bytes
    //!      (BitBlt from display DC — captures layered/overlay windows).
    //!   4. Decode the PNG and sample a 40×40 px patch centred on the cursor.
    //!   5. Assert the patch contains cursor-like pixels (bright or saturated).

    if !driver_binary().exists() {
        eprintln!("Binary not found — skipping");
        return;
    }
    if !require_seedable_desktop("agent cursor visibility") {
        return;
    }

    let Some(mut driver) = McpDriver::spawn() else {
        return;
    };

    // Safe centre-ish position on primary monitor.
    let cx = 640.0_f64;
    let cy = 400.0_f64;
    let cursor_id = "guard-ux-cursor";

    // Enable cursor overlay and glide to target.
    let r = driver.call(
        "set_agent_cursor_enabled",
        serde_json::json!({"enabled": true, "cursor_id": cursor_id}),
    );
    assert!(
        r.raw["error"].is_null(),
        "set_agent_cursor_enabled failed: {:?}",
        r.raw
    );

    let r = driver.call(
        "set_agent_cursor_motion",
        serde_json::json!({
            "cursor_id": cursor_id,
            "glide_duration_ms": 100,
            "idle_hide_ms": 0
        }),
    );
    assert!(
        r.raw["error"].is_null(),
        "set_agent_cursor_motion failed: {:?}",
        r.raw
    );

    let r = driver.call(
        "move_cursor",
        serde_json::json!({"x": cx, "y": cy, "cursor_id": cursor_id}),
    );
    assert!(r.raw["error"].is_null(), "move_cursor failed: {:?}", r.raw);

    // Wait for the fixed 100ms glide plus a few render frames.
    std::thread::sleep(Duration::from_millis(350));

    // Capture the screen directly (includes layered windows like the overlay).
    let png_bytes = platform_windows::capture::screenshot_display_bytes()
        .expect("screenshot_display_bytes failed");

    drop(driver);

    // Decode PNG.
    let img = image::load_from_memory(&png_bytes).expect("decode PNG");
    let rgba = img.to_rgba8();
    let (iw, ih) = rgba.dimensions();

    // Sample 40×40 patch centred on (cx, cy).
    let half = 20u32;
    let x0 = (cx as u32).saturating_sub(half).min(iw.saturating_sub(1));
    let x1 = (cx as u32 + half).min(iw);
    let y0 = (cy as u32).saturating_sub(half).min(ih.saturating_sub(1));
    let y1 = (cy as u32 + half).min(ih);

    let mut colourful_pixels = 0u32;
    for py in y0..y1 {
        for px in x0..x1 {
            let [r, g, b, a] = rgba.get_pixel(px, py).0;
            if a < 10 {
                continue;
            }
            let brightness = r as u32 + g as u32 + b as u32;
            let saturation = r.max(g).max(b) as u32 - r.min(g).min(b) as u32;
            // Accept bright-white stroke pixels OR coloured gradient pixels.
            if brightness > 60 && (saturation > 30 || brightness > 600) {
                colourful_pixels += 1;
            }
        }
    }

    assert!(
        colourful_pixels >= 5,
        "Agent cursor not visible at ({cx},{cy}): only {colourful_pixels} qualifying pixels \
         in 40×40 patch (x={x0}..{x1}, y={y0}..{y1}, image={iw}x{ih}). \
         Overlay may not be rendering or is positioned off-screen."
    );
}
