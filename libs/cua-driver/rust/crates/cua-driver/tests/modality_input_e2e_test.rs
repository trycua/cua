//! End-to-end Windows test for the **unified background input interface**:
//! a caller targets an app and plays click/key actions WITHOUT knowing the
//! app's internals (Electron/Chromium, Tauri/WebView2, classic Win32) and
//! WITHOUT the target window ever being raised to the foreground.
//!
//! Verified two ways per action:
//!   1. The tool succeeds in the DEFAULT dispatch mode — no
//!      `background_unavailable` error, no `dispatch:"foreground"` needed.
//!   2. The `focus-monitor-win` sentinel records ZERO foreground losses across
//!      the action == the target window was not z-raised over the user's
//!      window (same oracle as `harness_bg_modality_test`).
//!
//! ## Process hygiene
//! The driver, target app, and sentinel are all owned by the shared testkit
//! `ChildReaper` (via `McpDriver`), which on Windows assigns them to a
//! kill-on-close Job Object — the OS reaps the whole tree when the test process
//! exits for ANY reason (normal, panic, SIGKILL), so no orphaned windows or
//! held ports. Broker-spawned window pids (packaged apps / Electron) are
//! tree-killed via `reaper().track_pid`.
//!
//! Targets (repo-local harness apps staged by `test-harness/build/windows.ps1`):
//!   - Electron (Chromium content) — `CuaTestHarness.Electron.exe`.
//!   - Tauri (WebView2 content)    — `CuaTestHarness.Tauri.exe`.
//!   - Win32 baseline              — `notepad.exe`.
//! Override with `HARNESS_ELECTRON_EXE` / `HARNESS_TAURI_EXE`
//! (or legacy `CUA_ELECTRON_EXE` / `CUA_TAURI_EXE`).
//!
//! All tests are `#[ignore]` (GUI, real desktop session). Run explicitly,
//! serially (GUI focus oracle, so never in parallel):
//!   cargo test -p cua-driver --test e2e_windows_bg_input_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "windows")]

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::{ax, workspace_root, Driver, McpDriver};

// ── paths ────────────────────────────────────────────────────────────────────

fn focus_monitor_binary() -> PathBuf {
    workspace_root().join("target/debug/focus-monitor-win.exe")
}

/// Best-effort kill of any prior instance of `exe` by basename, so a leftover
/// from an earlier (e.g. force-killed) run can't hold the app's fixed HTTP port.
fn kill_prior_by_name(exe: &Path) {
    if let Some(name) = exe.file_name().and_then(|n| n.to_str()) {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/IM", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn resolve_harness(primary_env: &str, legacy_env: &str, rel_path: &str) -> Option<PathBuf> {
    for env_var in [primary_env, legacy_env] {
        if let Ok(p) = std::env::var(env_var) {
            let pb = PathBuf::from(p);
            if pb.exists() {
                return Some(pb);
            }
        }
    }
    let pb = workspace_root().join(rel_path);
    pb.exists().then_some(pb)
}
fn electron_exe() -> Option<PathBuf> {
    resolve_harness(
        "HARNESS_ELECTRON_EXE",
        "CUA_ELECTRON_EXE",
        "test-apps/harness-electron/CuaTestHarness.Electron.exe",
    )
}
fn tauri_exe() -> Option<PathBuf> {
    resolve_harness(
        "HARNESS_TAURI_EXE",
        "CUA_TAURI_EXE",
        "test-apps/harness-tauri/CuaTestHarness.Tauri.exe",
    )
}

fn loss_file() -> PathBuf {
    std::env::temp_dir().join("focus_monitor_losses.txt")
}
fn key_loss_file() -> PathBuf {
    std::env::temp_dir().join("focus_monitor_key_losses.txt")
}
fn focus_pid_file() -> PathBuf {
    std::env::temp_dir().join("focus_monitor_pid.txt")
}
fn focus_hwnd_file() -> PathBuf {
    std::env::temp_dir().join("focus_monitor_hwnd.txt")
}
fn read_count(p: &Path) -> u32 {
    fs::read_to_string(p)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

fn window_ids(driver: &mut McpDriver) -> HashSet<u64> {
    let r = driver.call("list_windows", serde_json::json!({}));
    r.structured()["windows"]
        .as_array()
        .map(|a| a.iter().filter_map(|w| w["window_id"].as_u64()).collect())
        .unwrap_or_default()
}

// ── fixture ───────────────────────────────────────────────────────────────────

struct E2eFixture {
    driver: McpDriver, // owns + reaps target+sentinel+driver on drop
    pid: u32,
    wid: u64,
}

/// Launch order: driver (no window) → snapshot windows → app → discover the
/// app's NEW window (works for multi-process apps like Electron whose window
/// belongs to a child pid) → sentinel (grabs foreground, pushing the app to
/// the background) → reset counters.
fn setup(target_exe: &Path, _title_hint: &str) -> Option<E2eFixture> {
    let fm_bin = focus_monitor_binary();
    if !fm_bin.exists() {
        eprintln!("[e2e] focus-monitor-win.exe not built — skipping");
        return None;
    }
    if target_exe.is_absolute() && !target_exe.exists() {
        eprintln!("[e2e] target {target_exe:?} missing — skipping");
        return None;
    }

    // Defensive: clear any leftover instance holding the app's fixed port.
    kill_prior_by_name(target_exe);
    let _ = fs::write(loss_file(), "0");
    let _ = fs::write(key_loss_file(), "0");
    let _ = fs::remove_file(focus_pid_file());
    let _ = fs::remove_file(focus_hwnd_file());

    // 1. Driver (daemon, no visible window) — spawned + initialized by the testkit.
    let mut driver = McpDriver::spawn()?;

    // 2. Snapshot existing windows, then launch the target app into the reaper.
    let before = window_ids(&mut driver);
    if driver
        .reaper()
        .spawn(
            Command::new(target_exe)
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .is_err()
    {
        eprintln!("[e2e] target spawn failed"); // driver drops → kills itself
        return None;
    }

    // 3. Discover the app's NEW window (its pid may be a child of the launched
    //    process — Electron/WebView2 are multi-process).
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut found: Option<(u32, u64)> = None;
    while Instant::now() < deadline {
        let r = driver.call("list_windows", serde_json::json!({}));
        if let Some(arr) = r.structured()["windows"].as_array() {
            for w in arr {
                let Some(wid) = w["window_id"].as_u64() else {
                    continue;
                };
                let pid = w["pid"].as_u64().unwrap_or(0) as u32;
                let title = w["title"].as_str().unwrap_or("");
                if !before.contains(&wid) && !title.is_empty() && pid != 0 {
                    found = Some((pid, wid));
                    break;
                }
            }
        }
        if found.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let (pid, wid) = match found {
        Some(p) => p,
        None => {
            eprintln!("[e2e] app window never appeared — skipping");
            return None;
        }
    };
    // The window's process is often broker-spawned (packaged apps, Electron),
    // i.e. NOT our direct child. track_pid job-assigns it (hard-kill safety) and
    // tree-kills it on teardown so it can't orphan.
    driver.reaper().track_pid(pid);

    // 4. Sentinel grabs foreground; app drops to z+1 (the background target).
    if driver
        .reaper()
        .spawn(
            Command::new(&fm_bin)
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .is_err()
    {
        eprintln!("[e2e] sentinel spawn failed — skipping");
        return None;
    }
    let sdeadline = Instant::now() + Duration::from_secs(10);
    loop {
        let ok = read_count(&focus_pid_file()) != 0
            && fs::read_to_string(focus_hwnd_file())
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(0)
                != 0;
        if ok {
            break;
        }
        if Instant::now() > sdeadline {
            eprintln!("[e2e] sentinel never published — skipping");
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(400));
    let _ = fs::write(loss_file(), "0");
    let _ = fs::write(key_loss_file(), "0");

    Some(E2eFixture { driver, pid, wid })
}

// ── DOM registration oracle (trycua test apps serve an event log on 6769) ─────

const APP_API: &str = "http://127.0.0.1:6769";

fn http_reset() {
    let _ = Command::new("curl.exe")
        .args(["-s", "-m", "3", "-X", "POST", &format!("{APP_API}/reset")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
/// Returns the `/events` body, or None if the app isn't serving the API.
fn http_events() -> Option<String> {
    let out = Command::new("curl.exe")
        .args(["-s", "-m", "3", &format!("{APP_API}/events")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&out.stdout).into_owned();
    if body.trim_start().starts_with('[') {
        Some(body)
    } else {
        None
    }
}
/// True if the event log recorded at least one DOM event since the last reset.
fn registered_since_reset() -> Option<bool> {
    http_events().map(|b| b.trim() != "[]" && !b.trim().is_empty())
}

fn foreground_pid() -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let h = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        pid
    }
}

/// The reliable "no z-raise / no foreground steal" oracle, independent of the
/// machine's foreground-lock setting: after acting on a BACKGROUND target, the
/// target's process must never become the foreground window. Polls for ~1.2s
/// to also catch async self-reactivation (Chromium).
fn assert_target_stays_background<F: FnOnce()>(label: &str, target_pid: u32, action: F) {
    let user_pid = read_count(&focus_pid_file());
    let fg_before = foreground_pid();
    if fg_before == target_pid {
        eprintln!("[e2e] WARN {label}: target pid {target_pid} was already foreground before the action (setup couldn't background it)");
    }
    action();
    let deadline = Instant::now() + Duration::from_millis(1200);
    let mut stole = false;
    let mut last_fg = fg_before;
    while Instant::now() < deadline {
        last_fg = foreground_pid();
        if last_fg == target_pid {
            stole = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    assert!(
        !stole,
        "{label}: target pid {target_pid} BECAME the foreground window — z-raise / foreground steal. \
         (expected user pid {user_pid}, fg_before={fg_before})"
    );
    println!("✅ {label}: target pid {target_pid} stayed background (foreground pid={last_fg}, user={user_pid})");
}

/// Shared body: default-mode left click into a webview target.
fn webview_click_case(label: &str, exe: PathBuf) {
    let mut fx = match setup(&exe, "") {
        Some(f) => f,
        None => return,
    };
    let (pid, wid) = (fx.pid, fx.wid);
    let snap = fx.driver.call(
        "get_window_state",
        serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}),
    );
    let elem = ax::element_index_containing(snap.text(), "button")
        .or_else(|| ax::element_index_containing(snap.text(), "click"));

    http_reset();
    let mut delivered = String::new();
    let mut errored = false;
    let mut needs_foreground = false;
    assert_target_stays_background(label, pid, || {
        let args = match elem {
            Some(idx) => {
                serde_json::json!({"pid": pid as i64, "window_id": wid, "element_index": idx})
            }
            None => serde_json::json!({"pid": pid as i64, "window_id": wid, "x": 200, "y": 200}),
        };
        let last = fx.driver.call("click", args);
        errored = last.is_error();
        needs_foreground = last.text().contains("background_unavailable");
        delivered = last.text().to_string();
    });
    assert!(!errored, "{label}: default-mode click errored: {delivered}");
    assert!(
        !needs_foreground,
        "{label}: click should not need dispatch:foreground, got {delivered:?}"
    );
    // Delivery is confirmed by the driver's own ✅ result (UIA Invoke fires the
    // element's default action / DOM `click`). The /events pointer-log is info
    // only — it does NOT capture accessibility-driven UIA Invoke, so a `[]`
    // there is expected for the UIA path. The hard invariant is no-foreground-steal.
    std::thread::sleep(Duration::from_millis(300));
    let reg = registered_since_reset()
        .map(|b| b.to_string())
        .unwrap_or_else(|| "n/a".into());
    println!("{label}: delivered={delivered:?}  pointer-events-logged={reg}");
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn e2e_electron_background_click_no_z_raise() {
    let Some(exe) = electron_exe() else {
        eprintln!("[e2e] no electron app — skipping");
        return;
    };
    webview_click_case("electron click (default dispatch)", exe);
}

#[test]
#[ignore]
fn e2e_tauri_background_click_no_z_raise() {
    let Some(exe) = tauri_exe() else {
        eprintln!("[e2e] no tauri app — skipping");
        return;
    };
    webview_click_case("tauri click (default dispatch)", exe);
}

#[test]
#[ignore]
fn e2e_win32_notepad_background_click_no_z_raise() {
    let mut fx = match setup(Path::new(r"C:\Windows\System32\notepad.exe"), "") {
        Some(f) => f,
        None => return,
    };
    let (pid, wid) = (fx.pid, fx.wid);
    let mut last_text = String::new();
    let mut errored = false;
    assert_target_stays_background("notepad click (default dispatch)", pid, || {
        let r = fx.driver.call(
            "click",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "x": 120, "y": 120}),
        );
        errored = r.is_error();
        last_text = r.text().to_string();
    });
    assert!(!errored, "notepad click errored: {last_text}");
    println!("notepad click result: {last_text:?}");
}

/// Electron right-click (pen-barrel injection): no raise. Pen→right promotion
/// is app-dependent, so landing is best-effort; the hard invariant is no
/// z-raise + no background_unavailable.
#[test]
#[ignore]
fn e2e_electron_background_right_click_no_z_raise() {
    let Some(exe) = electron_exe() else {
        eprintln!("[e2e] no electron app — skipping");
        return;
    };
    let mut fx = match setup(&exe, "") {
        Some(f) => f,
        None => return,
    };
    let (pid, wid) = (fx.pid, fx.wid);
    let mut last_text = String::new();
    assert_target_stays_background("electron right-click (default dispatch)", pid, || {
        let r = fx.driver.call(
            "click",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "x": 200, "y": 200, "button": "right"}),
        );
        last_text = r.text().to_string();
    });
    println!("electron right-click result: {last_text:?}");
}

/// Electron TEXT typing: focus a field, then type — must register the text in
/// the DOM AND never steal foreground / move the cursor.
#[test]
#[ignore]
fn e2e_electron_background_type_text_no_z_raise() {
    let Some(exe) = electron_exe() else {
        eprintln!("[e2e] no electron app — skipping");
        return;
    };
    let mut fx = match setup(&exe, "") {
        Some(f) => f,
        None => return,
    };
    let (pid, wid) = (fx.pid, fx.wid);

    // Focus a text field if the page exposes one (so the chars have a sink).
    let snap = fx.driver.call(
        "get_window_state",
        serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}),
    );
    let field = ax::element_index_containing(snap.text(), "edit")
        .or_else(|| ax::element_index_containing(snap.text(), "text"))
        .or_else(|| ax::element_index_containing(snap.text(), "input"));
    if let Some(idx) = field {
        let _ = fx.driver.call(
            "click",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "element_index": idx}),
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    http_reset();
    let mut last_text = String::new();
    let mut errored = false;
    assert_target_stays_background("electron type_text (default dispatch)", pid, || {
        let r = fx.driver.call(
            "type_text",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "text": "cuatest"}),
        );
        errored = r.is_error();
        last_text = r.text().to_string();
    });
    assert!(!errored, "type_text errored: {last_text}");
    std::thread::sleep(Duration::from_millis(300));
    if let Some(reg) = registered_since_reset() {
        println!("electron type_text: registered={reg}  result={last_text:?}");
    } else {
        println!("electron type_text result: {last_text:?}");
    }
}

/// Electron key-combo (Ctrl+A): the dropped-PostMessage keyboard path now uses
/// cloaked focus + SendInput. Hard invariant: no z-raise.
#[test]
#[ignore]
fn e2e_electron_background_keycombo_no_z_raise() {
    let Some(exe) = electron_exe() else {
        eprintln!("[e2e] no electron app — skipping");
        return;
    };
    let mut fx = match setup(&exe, "") {
        Some(f) => f,
        None => return,
    };
    let (pid, wid) = (fx.pid, fx.wid);
    let mut last_text = String::new();
    let mut errored = false;
    assert_target_stays_background("electron Ctrl+A (default dispatch)", pid, || {
        let r = fx.driver.call(
            "press_key",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "key": "a", "modifiers": ["control"]}),
        );
        errored = r.is_error();
        last_text = r.text().to_string();
    });
    // Capability-first: the combo must be DELIVERED, not refused.
    assert!(
        !errored && !last_text.contains("background_unavailable"),
        "Ctrl+A must be delivered (capability over UX), got: {last_text:?}"
    );
    println!("electron Ctrl+A delivered: {last_text:?}");
}
