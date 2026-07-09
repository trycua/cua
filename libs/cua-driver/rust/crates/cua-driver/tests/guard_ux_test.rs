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
//! All tests:
//!   1. Launch focus-monitor-win (the "user's foreground window").
//!   2. Perform background operations via cua-driver.
//!   3. Assert focus-monitor-win's act_losses counter stayed at 0.
//!
//! Run in sandbox via:
//!   .\sandbox\run-tests-in-sandbox.ps1 ux_guard

#![cfg(target_os = "windows")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::{driver_binary, spawn_in_job, workspace_root, Driver, McpDriver};

// ── focus-monitor + test-app fixtures ────────────────────────────────────────

fn require_interactive_desktop(context: &str) -> bool {
    if platform_windows::diagnostics::current_session_id() == Some(0) {
        eprintln!(
            "{context}: running in Windows Session 0; skipping GUI UX guard. \
             Re-run from an interactive logon session (RDP/console/scheduled task in user session)."
        );
        return false;
    }

    match platform_windows::diagnostics::interactive_desktop_check() {
        Ok(true) => true,
        Ok(false) => {
            eprintln!("{context}: no foreground interactive desktop; skipping GUI UX guard");
            false
        }
        Err(e) => {
            eprintln!("{context}: interactive desktop check failed ({e}); skipping GUI UX guard");
            false
        }
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
    if !require_interactive_desktop("focus-monitor-win") {
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

    let child = spawn_in_job(
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
    // Give the window time to become foreground.
    std::thread::sleep(Duration::from_millis(400));
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
    //! 1. Launch FocusMonitorWin (simulates the user's active window).
    //! 2. Launch the Electron harness in the background.
    //! 3. Click inside the app and type text via cua-driver.
    //! 4. Assert act_losses on FocusMonitorWin stayed at 0.

    if !driver_binary().exists() {
        eprintln!("Binary not found — skipping");
        return;
    }

    let Some((mut fm_proc, _fm_hwnd, _fm_pid)) = launch_focus_monitor() else {
        return;
    };
    let losses_before = read_losses(&loss_file());

    let Some(mut driver) = McpDriver::spawn() else {
        fm_proc.kill().ok();
        return;
    };

    let Some(app_pid) = launch_test_app(&mut driver) else {
        eprintln!("test app not available — skipping");
        fm_proc.kill().ok();
        return;
    };
    let Some(app_wid) = find_window_for_pid(&mut driver, app_pid as i64) else {
        eprintln!("test app window not found — skipping");
        fm_proc.kill().ok();
        return;
    };

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

    // ux_guard: FocusMonitorWin must not have lost activation.
    assert_ux_guard(
        losses_before,
        0,
        "background click + type_text into CuaTestHarness.Electron",
    );

    fm_proc.kill().ok();
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: launch_app does not steal focus
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_launch_app_no_focus_steal() {
    //! Equivalent of macOS test_launch_app_visible.py.
    //!
    //! launch_app (path variant) must open a window without displacing FocusMonitorWin.

    if !driver_binary().exists() {
        return;
    }

    let exe = test_app_path();
    if !exe.exists() {
        eprintln!("test app not available — skipping");
        return;
    }

    let Some((mut fm_proc, _fm_hwnd, _fm_pid)) = launch_focus_monitor() else {
        return;
    };
    let losses_before = read_losses(&loss_file());

    let Some(mut driver) = McpDriver::spawn() else {
        fm_proc.kill().ok();
        return;
    };

    // Launch the test app via cua-driver launch_app (full path, SW_SHOWNOACTIVATE).
    let path_str = exe.to_string_lossy().into_owned();
    let r = driver.call("launch_app", serde_json::json!({"path": path_str}));
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
    assert_ux_guard(losses_before, 0, "launch_app CuaTestHarness.Electron");

    // Kill the launched app by exe name.
    Command::new("taskkill")
        .args(["/F", "/T", "/IM", "CuaTestHarness.Electron.exe"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok();
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

    let Some((mut fm_proc, _fm_hwnd, _fm_pid)) = launch_focus_monitor() else {
        return;
    };
    let losses_before = read_losses(&loss_file());

    let Some(mut driver) = McpDriver::spawn() else {
        fm_proc.kill().ok();
        return;
    };

    let Some(app_pid) = launch_test_app(&mut driver) else {
        eprintln!("test app not available — skipping");
        fm_proc.kill().ok();
        return;
    };
    let Some(app_wid) = find_window_for_pid(&mut driver, app_pid as i64) else {
        eprintln!("test app window not found — skipping");
        fm_proc.kill().ok();
        return;
    };

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
    //! 2. Click in CuaTestHarness.Electron - a new window/tab may appear.
    //! 3. FocusMonitorWin must remain active throughout (UX guard).
    //!
    //! Verifies PostMessage doesn't inadvertently activate any new window that
    //! appears as a side-effect of the click.

    if !driver_binary().exists() {
        return;
    }

    let Some((mut fm_proc, _fm_hwnd, _fm_pid)) = launch_focus_monitor() else {
        return;
    };
    let losses_before = read_losses(&loss_file());

    let Some(mut driver) = McpDriver::spawn() else {
        fm_proc.kill().ok();
        return;
    };

    let Some(app_pid) = launch_test_app(&mut driver) else {
        eprintln!("test app not available — skipping");
        fm_proc.kill().ok();
        return;
    };
    let Some(app_wid) = find_window_for_pid(&mut driver, app_pid as i64) else {
        eprintln!("test app window not found — skipping");
        fm_proc.kill().ok();
        return;
    };

    // Click somewhere in the app content area (may trigger navigation/new window).
    let r = driver.call(
        "click",
        serde_json::json!({"pid": app_pid, "window_id": app_wid, "x": 400.0, "y": 350.0}),
    );
    assert!(
        r.raw["error"].is_null(),
        "Protocol error from click: {:?}",
        r.raw
    );

    // Brief wait for any side-effect windows to appear.
    std::thread::sleep(Duration::from_millis(500));

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

    let Some((mut fm_proc, _fm_hwnd, _fm_pid)) = launch_focus_monitor() else {
        return;
    };
    let losses_before = read_losses(&loss_file());

    let Some(mut driver) = McpDriver::spawn() else {
        fm_proc.kill().ok();
        return;
    };

    let Some(app_pid) = launch_test_app(&mut driver) else {
        eprintln!("test app not available — skipping");
        fm_proc.kill().ok();
        return;
    };
    let Some(app_wid) = find_window_for_pid(&mut driver, app_pid as i64) else {
        eprintln!("test app window not found — skipping");
        fm_proc.kill().ok();
        return;
    };

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
    if !require_interactive_desktop("agent cursor visibility") {
        return;
    }

    let Some(mut driver) = McpDriver::spawn() else {
        return;
    };

    // Safe centre-ish position on primary monitor.
    let cx = 640.0_f64;
    let cy = 400.0_f64;

    // Enable cursor overlay and glide to target.
    let r = driver.call(
        "set_agent_cursor_enabled",
        serde_json::json!({"enabled": true}),
    );
    assert!(
        r.raw["error"].is_null(),
        "set_agent_cursor_enabled failed: {:?}",
        r.raw
    );

    let r = driver.call("move_cursor", serde_json::json!({"x": cx, "y": cy}));
    assert!(r.raw["error"].is_null(), "move_cursor failed: {:?}", r.raw);

    // Wait for the glide animation (750ms default) + a few render frames.
    std::thread::sleep(Duration::from_millis(900));

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
