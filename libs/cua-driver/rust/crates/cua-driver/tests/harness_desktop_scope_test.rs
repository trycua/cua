//! Harness integration test for the **desktop-scope** modality (#1968 / #2019).
//!
//! Desktop-scope is cua-driver's *foreground*, vision-only, **screen-absolute**
//! loop (the "Computer-Use 1.0" mode), the complement to the default per-window
//! background model that `harness_bg_modality_test` / `e2e_windows_bg_input_test`
//! cover. This test exercises the Windows Phase-1 actuator end-to-end against a
//! real harness app:
//!
//!   1. `set_config capture_scope=desktop` → `get_desktop_state` returns a
//!      full-display capture with true `screen_width/height` (no downscale).
//!   2. A **window-less** screen-absolute `click` / `scroll` (no pid/window_id)
//!      lands via `WindowFromPoint` while in desktop scope.
//!   3. Negative gate: the same window-less `click` under `capture_scope=window`
//!      is rejected with the structured `desktop_scope_disabled` error.
//!
//! Note: `set_config` is a *session* override — it persists for the lifetime of
//! the one MCP server we spawn here (not across separate `cua-driver call`
//! processes), which is exactly why this test drives a single long-lived server.
//!
//! All tests are `#[ignore]` (need a real desktop session). Run explicitly:
//!   cargo test -p cua-driver --test harness_desktop_scope_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "windows")]

use core::ffi::c_void;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
    JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

const CALL_TIMEOUT: Duration = Duration::from_secs(25);

// ── kill-on-close job object (same pattern as e2e_windows_bg_input_test) ───────
static JOB: OnceLock<usize> = OnceLock::new();
fn job() -> HANDLE {
    let raw = *JOB.get_or_init(|| unsafe {
        let h = CreateJobObjectW(None, windows::core::PCWSTR::null()).expect("CreateJobObjectW");
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let _ = SetInformationJobObject(
            h, JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        h.0 as usize
    });
    HANDLE(raw as *mut c_void)
}
fn spawn_in_job(cmd: &mut Command) -> std::io::Result<Child> {
    let child = cmd.spawn()?;
    unsafe {
        let h = HANDLE(child.as_raw_handle() as *mut c_void);
        let _ = AssignProcessToJobObject(job(), h);
    }
    Ok(child)
}
fn assign_pid_to_job(pid: u32) {
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) {
            if !h.is_invalid() {
                let _ = AssignProcessToJobObject(job(), h);
                let _ = CloseHandle(h);
            }
        }
    }
}
struct ChildBag { children: Vec<Child>, pids: Vec<u32> }
impl ChildBag {
    fn new() -> Self { ChildBag { children: Vec::new(), pids: Vec::new() } }
    fn push(&mut self, c: Child) { self.children.push(c); }
    fn track_pid(&mut self, pid: u32) { self.pids.push(pid); }
}
impl Drop for ChildBag {
    fn drop(&mut self) {
        for pid in &self.pids {
            let _ = Command::new("taskkill").args(["/F", "/T", "/PID", &pid.to_string()])
                .stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
        for c in &mut self.children { let _ = c.kill(); let _ = c.wait(); }
        std::thread::sleep(Duration::from_millis(250));
    }
}

// ── paths ─────────────────────────────────────────────────────────────────────
fn workspace_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).parent().unwrap().parent().unwrap().to_owned()
}
fn driver_binary() -> PathBuf { workspace_root().join("target/debug/cua-driver.exe") }
/// WPF harness app (built by `test-harness/build/windows.ps1`). Path mirrors
/// `shared/scenarios.json`'s `wpf.exe_relative_path`.
fn harness_wpf_exe() -> PathBuf {
    workspace_root().join("test-apps/harness-wpf/CuaTestHarness.Wpf.exe")
}

// ── JSON-RPC over the driver's stdio ──────────────────────────────────────────
fn send(stdin: &mut ChildStdin, req: serde_json::Value) {
    let _ = writeln!(stdin, "{}", serde_json::to_string(&req).unwrap());
    let _ = stdin.flush();
}
fn call(stdin: &mut ChildStdin, rx: &Receiver<String>, id: u32, name: &str,
        args: serde_json::Value) -> serde_json::Value {
    send(stdin, serde_json::json!({
        "jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}
    }));
    match rx.recv_timeout(CALL_TIMEOUT) {
        Ok(line) => serde_json::from_str(&line)
            .unwrap_or_else(|_| serde_json::json!({"error": format!("bad json: {line}")})),
        Err(_) => serde_json::json!({"error": format!("TIMEOUT (>{}s) on {name}", CALL_TIMEOUT.as_secs())}),
    }
}
fn init(stdin: &mut ChildStdin, rx: &Receiver<String>) {
    send(stdin, serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    let _ = rx.recv_timeout(CALL_TIMEOUT);
}
fn result_text(s: &serde_json::Value) -> String {
    s["result"]["content"][0]["text"].as_str().unwrap_or("").to_string()
}
fn is_error(s: &serde_json::Value) -> bool {
    s["result"]["isError"].as_bool().unwrap_or(false) || s.get("error").is_some()
}
fn structured<'a>(s: &'a serde_json::Value) -> &'a serde_json::Value {
    &s["result"]["structuredContent"]
}

// ── fixture: one long-lived driver MCP server (session-scoped config) ─────────
struct Fixture { _bag: ChildBag, stdin: ChildStdin, rx: Receiver<String> }

fn spawn_driver() -> Option<Fixture> {
    let driver_bin = driver_binary();
    if !driver_bin.exists() {
        eprintln!("[desktop-scope] cua-driver.exe not built — skipping"); return None;
    }
    let mut bag = ChildBag::new();
    let mut driver = spawn_in_job(
        Command::new(&driver_bin).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
    ).inspect_err(|e| eprintln!("[desktop-scope] driver spawn failed: {e}")).ok()?;
    let mut stdin = driver.stdin.take().unwrap();
    let stdout = driver.stdout.take().unwrap();
    bag.push(driver);
    let (tx, rx) = channel::<String>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => { if tx.send(line.trim().to_string()).is_err() { break; } }
            }
        }
    });
    init(&mut stdin, &rx);
    Some(Fixture { _bag: bag, stdin, rx })
}

/// Launch the WPF harness app and return (pid, window bounds center in screen px).
/// Skips (returns None) if the harness app isn't built.
fn launch_wpf_and_center(fx: &mut Fixture) -> Option<(u32, i32, i32)> {
    let exe = harness_wpf_exe();
    if !exe.exists() {
        eprintln!("[desktop-scope] WPF harness not built ({exe:?}) — skipping window-target tests");
        return None;
    }
    let app = spawn_in_job(Command::new(&exe).stdout(Stdio::null()).stderr(Stdio::null())).ok()?;
    fx._bag.push(app);
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let r = call(&mut fx.stdin, &fx.rx, 50, "list_windows", serde_json::json!({}));
        if let Some(arr) = structured(&r)["windows"].as_array() {
            for w in arr {
                let title = w["title"].as_str().unwrap_or("");
                if !title.contains("CuaTestHarness") { continue; }
                let pid = w["pid"].as_u64().unwrap_or(0) as u32;
                // #2018: bounds is nested {x,y,width,height} on Windows.
                let b = &w["bounds"];
                let (x, y, ww, h) = (
                    b["x"].as_i64().unwrap_or(0) as i32, b["y"].as_i64().unwrap_or(0) as i32,
                    b["width"].as_i64().unwrap_or(0) as i32, b["height"].as_i64().unwrap_or(0) as i32,
                );
                if pid != 0 && ww > 0 && h > 0 {
                    assign_pid_to_job(pid);
                    fx._bag.track_pid(pid);
                    return Some((pid, x + ww / 2, y + h / 2));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    eprintln!("[desktop-scope] WPF harness window never appeared — skipping");
    None
}

fn set_scope(fx: &mut Fixture, scope: &str) {
    let r = call(&mut fx.stdin, &fx.rx, 10, "set_config",
        serde_json::json!({"key": "capture_scope", "value": scope}));
    assert!(!is_error(&r), "set_config capture_scope={scope} failed: {r}");
    assert_eq!(structured(&r)["capture_scope"].as_str(), Some(scope),
        "set_config did not report capture_scope={scope}: {r}");
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `get_desktop_state` in desktop scope returns a full-display capture with
/// real screen dimensions (the Session-0 `handle is invalid` case is only the
/// service-session wall; this needs a real interactive desktop).
#[test]
#[ignore]
fn desktop_scope_capture_returns_screen_dims() {
    let Some(mut fx) = spawn_driver() else { return };
    set_scope(&mut fx, "desktop");
    let r = call(&mut fx.stdin, &fx.rx, 20, "get_desktop_state", serde_json::json!({}));
    assert!(!is_error(&r), "get_desktop_state errored: {r}");
    let sw = structured(&r)["screen_width"].as_u64().unwrap_or(0);
    let sh = structured(&r)["screen_height"].as_u64().unwrap_or(0);
    assert!(sw > 0 && sh > 0, "get_desktop_state returned no/zero screen size: {r}");
    eprintln!("[desktop-scope] get_desktop_state OK — screen {sw}x{sh}");
}

/// In desktop scope, a window-less screen-absolute click + scroll succeed and
/// resolve a real window via WindowFromPoint (no pid/window_id supplied).
#[test]
#[ignore]
fn desktop_scope_windowless_click_and_scroll_land() {
    let Some(mut fx) = spawn_driver() else { return };
    set_scope(&mut fx, "desktop");
    let Some((_pid, cx, cy)) = launch_wpf_and_center(&mut fx) else { return };

    let clicked = call(&mut fx.stdin, &fx.rx, 21, "click", serde_json::json!({"x": cx, "y": cy}));
    assert!(!is_error(&clicked), "desktop-scope click errored: {clicked}");
    let ct = result_text(&clicked).to_lowercase();
    assert!(ct.contains("desktop scope"), "click not reported as desktop-scope: {}", result_text(&clicked));
    assert!(ct.contains("hwnd"), "click did not resolve a window via WindowFromPoint: {}", result_text(&clicked));

    let scrolled = call(&mut fx.stdin, &fx.rx, 22, "scroll",
        serde_json::json!({"x": cx, "y": cy, "direction": "down"}));
    assert!(!is_error(&scrolled), "desktop-scope scroll errored: {scrolled}");
    assert!(result_text(&scrolled).to_lowercase().contains("desktop scope"),
        "scroll not reported as desktop-scope: {}", result_text(&scrolled));
}

/// Negative gate: a window-less screen-absolute click under `capture_scope=window`
/// must be rejected (the `desktop_scope_disabled` contract), not silently retargeted.
#[test]
#[ignore]
fn window_scope_rejects_windowless_click() {
    let Some(mut fx) = spawn_driver() else { return };
    set_scope(&mut fx, "window");
    let r = call(&mut fx.stdin, &fx.rx, 30, "click", serde_json::json!({"x": 100, "y": 100}));
    let txt = result_text(&r).to_lowercase();
    assert!(
        is_error(&r) || txt.contains("desktop scope") || txt.contains("desktop_scope_disabled"),
        "window-scope window-less click was NOT rejected: {r}"
    );
}
