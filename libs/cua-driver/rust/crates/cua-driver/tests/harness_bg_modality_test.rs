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
//! Also covers **capture_mode ax** (UIA-only, no screenshot) and
//! **capture_mode vision** (screenshot-only, no UIA tree). The default
//! `som` covers both.

#![cfg(target_os = "windows")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

// ── paths ────────────────────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).parent().unwrap().parent().unwrap().to_owned()
}
fn driver_binary() -> PathBuf { workspace_root().join("target/debug/cua-driver.exe") }
fn focus_monitor_binary() -> PathBuf { workspace_root().join("target/debug/focus-monitor-win.exe") }
fn harness_wpf_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_WPF_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() { return pb; }
    }
    workspace_root().join("test-apps/harness-wpf/CuaTestHarness.Wpf.exe")
}

fn loss_file()      -> PathBuf { std::env::temp_dir().join("focus_monitor_losses.txt") }
fn gain_file()      -> PathBuf { std::env::temp_dir().join("focus_monitor_gains.txt") }
fn key_loss_file()  -> PathBuf { std::env::temp_dir().join("focus_monitor_key_losses.txt") }
fn key_gain_file()  -> PathBuf { std::env::temp_dir().join("focus_monitor_key_gains.txt") }
fn focus_pid_file()  -> PathBuf { std::env::temp_dir().join("focus_monitor_pid.txt") }
fn focus_hwnd_file() -> PathBuf { std::env::temp_dir().join("focus_monitor_hwnd.txt") }

fn read_count(p: &std::path::Path) -> u32 {
    std::fs::read_to_string(p).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

// ── JSON-RPC ─────────────────────────────────────────────────────────────────

fn send(stdin: &mut ChildStdin, req: serde_json::Value) {
    writeln!(stdin, "{}", serde_json::to_string(&req).unwrap()).unwrap();
}
fn recv(stdout: &mut BufReader<&mut ChildStdout>) -> serde_json::Value {
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read");
    serde_json::from_str(line.trim()).expect("json")
}
fn init(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>) {
    send(stdin, serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    let _ = recv(stdout);
}
fn call(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
        id: u32, name: &str, args: serde_json::Value) -> serde_json::Value {
    send(stdin, serde_json::json!({
        "jsonrpc":"2.0","id":id,"method":"tools/call",
        "params":{"name":name,"arguments":args}
    }));
    recv(stdout)
}

fn snapshot_text(s: &serde_json::Value) -> &str {
    s["result"]["content"][0]["text"].as_str().unwrap_or("")
}
fn find_idx_by_aid(s: &serde_json::Value, aid: &str) -> Option<u64> {
    let needle = format!("id={aid}");
    for line in snapshot_text(s).lines() {
        if !line.contains(&needle) { continue; }
        let st = line.find('[')? + 1;
        let en = line[st..].find(']')? + st;
        return line[st..en].trim().parse().ok();
    }
    None
}

// ── shared fixture ───────────────────────────────────────────────────────────

struct BgFixture {
    harness: Child,
    fm: Child,
    driver: Child,
    driver_stdin: ChildStdin,
    driver_stdout: ChildStdout,
    harness_pid: u32,
    harness_wid: u64,
}

impl Drop for BgFixture {
    fn drop(&mut self) {
        let _ = self.driver.kill();
        let _ = self.driver.wait();
        let _ = self.harness.kill();
        let _ = self.harness.wait();
        let _ = self.fm.kill();
        let _ = self.fm.wait();
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Launch sequence:
///  1. WPF harness (becomes foreground briefly on its own activation)
///  2. focus-monitor-win sentinel — its OnLoad SetForegroundWindow displaces
///     the harness; sentinel is now z+0, harness z+1.
///  3. cua-driver child
///  4. Reset losses.txt to 0 (sentinel may have logged its own startup activate)
fn setup() -> Option<BgFixture> {
    let driver_bin = driver_binary();
    if !driver_bin.exists() {
        eprintln!("cua-driver.exe not built — skipping"); return None;
    }
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

    let harness = Command::new(&h_exe)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().ok()?;
    let harness_pid = harness.id();
    std::thread::sleep(Duration::from_secs(1));

    // Launch sentinel. focus-monitor-win activates its own window which
    // displaces the harness.
    let fm = Command::new(&fm_bin)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().ok()?;
    // Wait for sentinel to publish its pid+hwnd files (so we know it's
    // up and has claimed foreground).
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

    let mut driver = Command::new(&driver_bin)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().ok()?;
    let mut driver_stdin = driver.stdin.take().unwrap();
    let driver_stdout_raw = driver.stdout.take().unwrap();
    let mut driver_stdout = driver_stdout_raw;
    {
        let mut stdout = BufReader::new(&mut driver_stdout);
        init(&mut driver_stdin, &mut stdout);
        let resp = call(&mut driver_stdin, &mut stdout, 10, "list_windows",
            serde_json::json!({"pid": harness_pid as i64}));
        let wid = resp["result"]["structuredContent"]["windows"].as_array()
            .and_then(|a| a.iter().find_map(|w| {
                if w["pid"].as_u64() == Some(harness_pid as u64)
                    && w["title"].as_str().map(|t| t.contains("CuaTestHarness WPF")).unwrap_or(false)
                { w["window_id"].as_u64() } else { None }
            }))
            .expect("harness window not found");
        drop(stdout);
        return Some(BgFixture {
            harness, fm, driver, driver_stdin, driver_stdout,
            harness_pid, harness_wid: wid,
        });
    }
}

fn with_session<F, R>(fx: &mut BgFixture, f: F) -> R
where F: FnOnce(&mut ChildStdin, &mut BufReader<&mut ChildStdout>, u32, u64) -> R {
    let mut stdout = BufReader::new(&mut fx.driver_stdout);
    f(&mut fx.driver_stdin, &mut stdout, fx.harness_pid, fx.harness_wid)
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

#[test]
#[ignore]
fn bg_modality_get_window_state_no_focus_steal() {
    let mut fx = match setup() { Some(f) => f, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        assert_no_focus_steal("get_window_state(som)", || {
            let _ = call(stdin, stdout, 30, "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "som"}));
        });
    });
}

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
fn bg_modality_uia_invoke_click_DOCUMENTED_steals_focus() {
    let mut fx = match setup() { Some(f) => f, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        let _ = call(stdin, stdout, 29, "set_agent_cursor_enabled",
            serde_json::json!({"enabled": false}));
        std::thread::sleep(Duration::from_millis(200));

        let snap = call(stdin, stdout, 30, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let idx = find_idx_by_aid(&snap, "btn-increment").expect("btn-increment");

        let before = read_count(&loss_file());
        let _ = call(stdin, stdout, 31, "click",
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
    });
}

/// Companion gap: UIA ValuePattern.SetValue on WPF TextBox.
/// Same root cause and mitigation path as
/// bg_modality_uia_invoke_click_DOCUMENTED_steals_focus.
#[test]
#[ignore]
fn bg_modality_set_value_DOCUMENTED_steals_focus() {
    let mut fx = match setup() { Some(f) => f, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        let _ = call(stdin, stdout, 29, "set_agent_cursor_enabled",
            serde_json::json!({"enabled": false}));
        std::thread::sleep(Duration::from_millis(200));

        let snap = call(stdin, stdout, 30, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let idx = find_idx_by_aid(&snap, "txt-input").expect("txt-input");

        let before = read_count(&loss_file());
        let _ = call(stdin, stdout, 31, "set_value",
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
    });
}

#[test]
#[ignore]
fn bg_modality_press_key_no_focus_steal() {
    let mut fx = match setup() { Some(f) => f, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        // F5 fires the harness accelerator (KeyBinding) via PostMessage
        // WM_KEYDOWN — background path, no foreground swap.
        assert_no_focus_steal("press_key(f5, PostMessage)", || {
            let _ = call(stdin, stdout, 30, "press_key",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "key": "f5"}));
        });
    });
}

#[test]
#[ignore]
fn bg_modality_scroll_no_focus_steal() {
    let mut fx = match setup() { Some(f) => f, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        assert_no_focus_steal("scroll(down, PostMessage WM_VSCROLL)", || {
            let _ = call(stdin, stdout, 30, "scroll",
                serde_json::json!({
                    "pid": pid as i64, "window_id": wid,
                    "direction": "down", "by": "line", "amount": 3
                }));
        });
    });
}

// ── CAPTURE MODE: ax + vision modalities ────────────────────────────────────

#[test]
#[ignore]
fn capture_mode_ax_returns_tree_only() {
    let mut fx = match setup() { Some(f) => f, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        let resp = call(stdin, stdout, 30, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        // ax mode: tree_markdown present, no image content array entry.
        let text = snapshot_text(&resp);
        assert!(text.contains("id=btn-increment"),
            "capture_mode=ax tree missing btn-increment AID");
        let has_image = resp["result"]["content"].as_array()
            .map(|a| a.iter().any(|c| c["type"].as_str() == Some("image")))
            .unwrap_or(false);
        assert!(!has_image,
            "capture_mode=ax should NOT return image content (got one anyway)");
        println!("✅ capture_mode_ax_returns_tree_only: tree present, no image");

        // And ax must not steal focus.
        assert_no_focus_steal("get_window_state(ax)", || {
            let _ = call(stdin, stdout, 31, "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        });
    });
}

#[test]
#[ignore]
fn capture_mode_vision_returns_image_only() {
    let mut fx = match setup() { Some(f) => f, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        let resp = call(stdin, stdout, 30, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "vision"}));
        // vision mode: image content present, no tree markdown.
        let has_image = resp["result"]["content"].as_array()
            .map(|a| a.iter().any(|c| c["type"].as_str() == Some("image")))
            .unwrap_or(false);
        assert!(has_image, "capture_mode=vision should return image content");

        let text_first = snapshot_text(&resp);
        // Tree markdown's hallmark is lines starting with `- [N] ` for elements.
        // In vision mode, those should be absent.
        let has_tree_markers = text_first.lines()
            .any(|l| l.trim_start().starts_with("- [") && l.contains(']'));
        assert!(!has_tree_markers,
            "capture_mode=vision should not return UIA tree markdown");
        println!("✅ capture_mode_vision_returns_image_only: image present, no tree");

        assert_no_focus_steal("get_window_state(vision)", || {
            let _ = call(stdin, stdout, 31, "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "vision"}));
        });
    });
}

#[test]
#[ignore]
fn capture_mode_ax_and_vision_invoke_roundtrip() {
    // Cross-modality round-trip: ax to find element, vision to confirm
    // the screenshot reflects the post-action state. Mirrors how an agent
    // alternates between symbolic and pixel views during a task.
    let mut fx = match setup() { Some(f) => f, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        let snap_ax = call(stdin, stdout, 30, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let idx = find_idx_by_aid(&snap_ax, "btn-increment").expect("btn-increment");

        let _ = call(stdin, stdout, 31, "click",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "element_index": idx}));
        std::thread::sleep(Duration::from_millis(300));

        let snap_vision = call(stdin, stdout, 32, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "vision"}));
        let has_image = snap_vision["result"]["content"].as_array()
            .map(|a| a.iter().any(|c| c["type"].as_str() == Some("image")))
            .unwrap_or(false);
        assert!(has_image, "vision snapshot didn't return an image");

        // And confirm counter advanced via a follow-up ax snapshot.
        let snap_ax2 = call(stdin, stdout, 33, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        assert!(snapshot_text(&snap_ax2).contains("counter=1"),
            "counter didn't advance after UIA Invoke");
        println!("✅ capture_mode_ax_and_vision_invoke_roundtrip: ax→invoke→vision+ax green");
    });
}
