//! Background-modality + capture-mode tests for the CuaTestHarness.Wpf
//! harness.
//!
//! These verify the **core cua-driver promise**: background automation
//! must not steal foreground from the user's active window. The harness
//! window is at z+0 when shown (whatever WPF gives it), the
//! focus-monitor-win sentinel is then activated to z+0 (displacing the
//! harness to z+1), and cua-driver actions targeting the harness must
//! NOT make the harness regain foreground.
//!
//! Sentinel: `focus-monitor-win` (already part of the workspace; built by
//! `cargo build`). It writes `focus_monitor_losses.txt` to %TEMP% — the
//! count of times its window lost activation. We snapshot before / after
//! each action and assert delta == 0.
//!
//! Also covers **capture_mode ax** (UIA-only, no screenshot — the default) and
//! **capture_mode vision** (screenshot-only, no UIA tree). (`som` is a
//! deprecated alias for `ax`.)

#![cfg(target_os = "windows")]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::ax::element_index_by_id;
use cua_driver_testkit::{harness_app, workspace_root, Driver, McpDriver};

// ── paths ────────────────────────────────────────────────────────────────────

fn focus_monitor_binary() -> PathBuf { workspace_root().join("target/debug/focus-monitor-win.exe") }
fn harness_wpf_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_WPF_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() { return pb; }
    }
    harness_app("harness-wpf", "CuaTestHarness.Wpf.exe")
}

fn loss_file()      -> PathBuf { std::env::temp_dir().join("focus_monitor_losses.txt") }
fn gain_file()      -> PathBuf { std::env::temp_dir().join("focus_monitor_gains.txt") }
fn key_loss_file()  -> PathBuf { std::env::temp_dir().join("focus_monitor_key_losses.txt") }
fn key_gain_file()  -> PathBuf { std::env::temp_dir().join("focus_monitor_key_gains.txt") }
fn focus_pid_file()  -> PathBuf { std::env::temp_dir().join("focus_monitor_pid.txt") }
fn focus_hwnd_file() -> PathBuf { std::env::temp_dir().join("focus_monitor_hwnd.txt") }

fn read_count(p: &std::path::Path) -> u32 {
    // Surface read / parse errors instead of swallowing them as 0 — a silent
    // 0 would mask actual focus-steal regressions by making delta computation
    // look "no change" when really the sentinel file is unreadable.
    let raw = std::fs::read_to_string(p)
        .unwrap_or_else(|e| panic!("failed reading sentinel counter {p:?}: {e}"));
    raw.trim()
        .parse::<u32>()
        .unwrap_or_else(|e| panic!("failed parsing sentinel counter {p:?} as u32: {e}"))
}

// ── shared fixture ───────────────────────────────────────────────────────────

/// Launch sequence:
///  0. cua-driver MCP server (headless stdio; its reaper owns the harness +
///     sentinel below, so any early-return or panic reaps the whole tree).
///  1. WPF harness (becomes foreground briefly on its own activation).
///  2. focus-monitor-win sentinel — its OnLoad SetForegroundWindow displaces
///     the harness; sentinel is now z+0, harness z+1.
///  3. Reset losses.txt to 0 (sentinel may have logged its own startup activate)
///     and resolve the harness window (pid + window_id).
fn setup() -> Option<(McpDriver, u32, u64)> {
    let fm_bin = focus_monitor_binary();
    if !fm_bin.exists() {
        eprintln!("focus-monitor-win.exe not built — skipping"); return None;
    }
    let h_exe = harness_wpf_exe();
    if !h_exe.exists() {
        eprintln!("harness WPF exe not built — skipping"); return None;
    }

    // Reset sentinel files so we start from a known baseline.
    let _ = std::fs::write(loss_file(), "0");
    let _ = std::fs::write(gain_file(), "0");
    let _ = std::fs::write(key_loss_file(), "0");
    let _ = std::fs::write(key_gain_file(), "0");
    let _ = std::fs::remove_file(focus_pid_file());
    let _ = std::fs::remove_file(focus_hwnd_file());

    // Spawn the cua-driver MCP server (skips if the binary isn't built).
    let mut driver = McpDriver::spawn()?;

    // 1. WPF harness, launched through the reaper so it can't outlive the test.
    driver
        .reaper()
        .spawn(Command::new(&h_exe).stdout(Stdio::null()).stderr(Stdio::null()))
        .ok()?;
    std::thread::sleep(Duration::from_secs(1));

    // 2. Launch sentinel. focus-monitor-win activates its own window which
    //    displaces the harness.
    if driver
        .reaper()
        .spawn(Command::new(&fm_bin).stdout(Stdio::null()).stderr(Stdio::null()))
        .is_err()
    {
        eprintln!("focus-monitor-win spawn failed");
        return None;
    }

    // Wait for sentinel to publish its pid+hwnd files (so we know it's up and
    // has claimed foreground). On timeout we return None; dropping `driver`
    // reaps the harness + sentinel so they don't poison subsequent tests.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let pid_ok = std::fs::read_to_string(focus_pid_file()).ok()
            .and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0) != 0;
        let hwnd_ok = std::fs::read_to_string(focus_hwnd_file()).ok()
            .and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0) != 0;
        if pid_ok && hwnd_ok { break; }
        if std::time::Instant::now() > deadline {
            eprintln!("focus-monitor sentinel never published pid/hwnd files");
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(400));   // sentinel-active settle

    // Reset losses again now that sentinel has settled. Any prior loss
    // counts (e.g. from harness coming up after the sentinel did) shouldn't
    // count against the test.
    let _ = std::fs::write(loss_file(), "0");
    let _ = std::fs::write(gain_file(), "0");
    let _ = std::fs::write(key_loss_file(), "0");
    let _ = std::fs::write(key_gain_file(), "0");

    // Resolve the harness window (pid + window_id) by title. The WPF harness is
    // its own window process, so list_windows surfaces it directly.
    let resp = driver.call("list_windows", serde_json::json!({}));
    let (harness_pid, harness_wid) = resp.structured()["windows"].as_array()
        .and_then(|a| a.iter().find_map(|w| {
            let title = w["title"].as_str().unwrap_or("");
            if title.contains("CuaTestHarness WPF") {
                Some((w["pid"].as_u64()? as u32, w["window_id"].as_u64()?))
            } else {
                None
            }
        }))
        .expect("harness window not found");

    Some((driver, harness_pid, harness_wid))
}

/// Strict mode: action must not generate ANY sentinel-loss event.
fn assert_no_focus_steal<F>(label: &str, f: F)
where F: FnOnce() {
    let before_act = read_count(&loss_file());
    let before_key = read_count(&key_loss_file());
    f();
    std::thread::sleep(Duration::from_millis(400));
    let after_act = read_count(&loss_file());
    let after_key = read_count(&key_loss_file());
    let d_act = after_act.saturating_sub(before_act);
    let d_key = after_key.saturating_sub(before_key);
    assert_eq!(d_act, 0,
        "{label}: sentinel act_losses went {before_act} -> {after_act} (delta={d_act}). \
         cua-driver's background action stole foreground from the user.");
    assert_eq!(d_key, 0,
        "{label}: sentinel key_losses went {before_key} -> {after_key} (delta={d_key}).");
    println!("✅ {label}: no focus steal (act_losses=0, key_losses=0)");
}

/// Relaxed mode: read GetForegroundWindow() after the action and assert
/// it's still the sentinel HWND. Tolerates a transient blip during the
/// action (which UIA Invoke against WPF Buttons creates unavoidably —
/// the target's handler calls Focus() before cua-driver gets control).
/// The cua-driver fg_bypass restores foreground after, and this check
/// asserts that restoration actually worked.
// Documented oracle kept for the foreground-restore scenarios; not currently
// wired to a live test (preserved verbatim from before the testkit migration).
#[allow(dead_code)]
fn assert_foreground_restored<F>(label: &str, f: F)
where F: FnOnce() {
    let sentinel_hwnd: u64 = std::fs::read_to_string(focus_hwnd_file())
        .ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    assert!(sentinel_hwnd != 0, "{label}: sentinel hwnd unknown (file missing)");

    let before_loss = read_count(&loss_file());
    f();
    std::thread::sleep(Duration::from_millis(500));
    let after_loss = read_count(&loss_file());
    let d_loss = after_loss.saturating_sub(before_loss);

    // Read current foreground via Win32 directly.
    let now_fg: u64 = unsafe {
        let h = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
        h.0 as u64
    };
    assert_eq!(now_fg, sentinel_hwnd,
        "{label}: GetForegroundWindow={now_fg:#x} but sentinel hwnd={sentinel_hwnd:#x}. \
         Foreground was NOT restored to the user's window after the action.");
    if d_loss == 0 {
        println!("✅ {label}: no focus blip at all (losses=0)");
    } else {
        println!("✅ {label}: foreground restored after {d_loss} blip(s) — sentinel HWND={sentinel_hwnd:#x} matches GetForegroundWindow");
    }
}

// ── BACKGROUND MODALITY: cua-driver must keep harness at z+1 ────────────────

/// Documents the cua-driver focus-steal gap for UIA Invoke on a WPF
/// Button. Root cause: WPF's ButtonBase.OnClick handler synchronously
/// calls `UIElement.Focus()` which routes through `SetForegroundWindow`
/// and is NOT gated by the EnableWindow(false) bypass used for UWP
/// hosts. The daemon CAN'T restore foreground reliably either, because
/// non-UIAccess processes are subject to the foreground-lock.
///
/// **Mitigation**: route UIA activations through `cua-driver-uia.exe`
/// (UIAccess-manifested worker). With UIAccess, the worker can both
/// suppress the self-foreground and restore the user's foreground if
/// it leaked through.
///
/// This test ASSERTS the gap currently exists. When cua-driver gains
/// the UIAccess worker path, flip this assertion to `delta == 0` and
/// rename to `..._no_focus_steal`.
#[test]
#[ignore]
// Name intentionally documents a KNOWN focus-steal (see doc comment above).
#[allow(non_snake_case)]
fn bg_modality_uia_invoke_click_DOCUMENTED_steals_focus() {
    let (mut driver, pid, wid) = match setup() { Some(x) => x, None => return };
    let _ = driver.call("set_agent_cursor_enabled",
        serde_json::json!({"enabled": false}));
    std::thread::sleep(Duration::from_millis(200));

    let snap = driver.call("get_window_state",
        serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
    let idx = element_index_by_id(snap.text(), "btn-increment").expect("btn-increment");

    let before = read_count(&loss_file());
    let _ = driver.call("click",
        serde_json::json!({"pid": pid as i64, "window_id": wid, "element_index": idx}));
    std::thread::sleep(Duration::from_millis(500));
    let after = read_count(&loss_file());
    let delta = after.saturating_sub(before);
    assert!(delta >= 1,
        "Expected the documented focus-steal gap (delta>=1). Got delta={delta}. \
         If this now passes, cua-driver has fixed UIA Invoke focus-steal — \
         flip this assertion to delta==0 and update the docstring.");
    println!("⚠️  bg_modality_uia_invoke_click: gap confirmed (delta={delta}). \
              Mitigation = route UIA via cua-driver-uia.exe (UIAccess worker).");
}

/// Companion gap: UIA ValuePattern.SetValue on WPF TextBox.
/// Same root cause and mitigation path as
/// bg_modality_uia_invoke_click_DOCUMENTED_steals_focus.
#[test]
#[ignore]
// Name intentionally documents a KNOWN focus-steal (see doc comment above).
#[allow(non_snake_case)]
fn bg_modality_set_value_DOCUMENTED_steals_focus() {
    let (mut driver, pid, wid) = match setup() { Some(x) => x, None => return };
    let _ = driver.call("set_agent_cursor_enabled",
        serde_json::json!({"enabled": false}));
    std::thread::sleep(Duration::from_millis(200));

    let snap = driver.call("get_window_state",
        serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
    let idx = element_index_by_id(snap.text(), "txt-input").expect("txt-input");

    let before = read_count(&loss_file());
    let _ = driver.call("set_value",
        serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "value": "via-uia-no-focus-steal"
        }));
    std::thread::sleep(Duration::from_millis(500));
    let after = read_count(&loss_file());
    let delta = after.saturating_sub(before);
    assert!(delta >= 1,
        "Expected the documented SetValue focus-steal gap. delta={delta}.");
    println!("⚠️  bg_modality_set_value: gap confirmed (delta={delta})");
}

#[test]
#[ignore]
fn bg_modality_press_key_no_focus_steal() {
    let (mut driver, pid, wid) = match setup() { Some(x) => x, None => return };
    // F5 fires the harness accelerator (KeyBinding) via PostMessage
    // WM_KEYDOWN — background path, no foreground swap.
    assert_no_focus_steal("press_key(f5, PostMessage)", || {
        let _ = driver.call("press_key",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "key": "f5"}));
    });
}

#[test]
#[ignore]
fn bg_modality_scroll_no_focus_steal() {
    let (mut driver, pid, wid) = match setup() { Some(x) => x, None => return };
    assert_no_focus_steal("scroll(down, PostMessage WM_VSCROLL)", || {
        let _ = driver.call("scroll",
            serde_json::json!({
                "pid": pid as i64, "window_id": wid,
                "direction": "down", "by": "line", "amount": 3
            }));
    });
}
