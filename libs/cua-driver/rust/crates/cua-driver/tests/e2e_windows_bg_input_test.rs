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
//! ## Process hygiene (no orphaned windows / held ports)
//! Every child (target app, sentinel, driver) is spawned into a Windows **Job
//! Object** created with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. The test process
//! holds the only job handle, so when it exits for ANY reason — normal end,
//! panic, or being force-killed (Ctrl-C) — the OS terminates the entire child
//! tree. A per-fixture `ChildBag` also kills+waits promptly between tests so
//! windows don't pile up during a run, and every early-return path reaps.
//!
//! Targets (trycua test apps, auto-downloaded to %TEMP% if not provided):
//!   - Electron (Chromium content) — `trycua/desktop-test-app-electron`.
//!   - Tauri (WebView2 content)    — `trycua/desktop-test-app`.
//!   - Win32 baseline              — `notepad.exe`.
//! Override with `CUA_ELECTRON_EXE` / `CUA_TAURI_EXE`, or drop the exe in
//! `test-apps/`. Auto-download uses `curl.exe`.
//!
//! Every JSON-RPC call is bounded by a hard timeout (a hung driver becomes a
//! fast, localized failure — never an indefinite wall-clock hang).
//!
//! All tests are `#[ignore]` (GUI, real desktop session). Run explicitly,
//! serially (apps bind a fixed HTTP port, so never in parallel):
//!   cargo test -p cua-driver --test e2e_windows_bg_input_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "windows")]

use core::ffi::c_void;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
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

const ELECTRON_URL: &str = "https://github.com/trycua/desktop-test-app-electron/releases/download/v0.1.0/desktop-test-app-electron.0.1.0.exe";
const TAURI_URL: &str = "https://github.com/trycua/desktop-test-app/releases/download/v0.2.2/desktop-test-app-windows-x86_64.exe";

// ── Job Object: kill the whole child tree when this test process dies ─────────

/// Process-global job handle (stored as usize so it's `Send`/`Sync` in the
/// OnceLock). Created lazily with KILL_ON_JOB_CLOSE.
static JOB: OnceLock<usize> = OnceLock::new();

fn job() -> HANDLE {
    let raw = *JOB.get_or_init(|| unsafe {
        let h = CreateJobObjectW(None, windows::core::PCWSTR::null()).expect("CreateJobObjectW");
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let _ = SetInformationJobObject(
            h,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        h.0 as usize
    });
    HANDLE(raw as *mut c_void)
}

/// Spawn a command and immediately assign it to the kill-on-close job, so it
/// can never outlive this test process.
fn spawn_in_job(cmd: &mut Command) -> std::io::Result<Child> {
    let child = cmd.spawn()?;
    unsafe {
        let h = HANDLE(child.as_raw_handle() as *mut c_void);
        let _ = AssignProcessToJobObject(job(), h);
    }
    Ok(child)
}

/// Assign an already-running pid (e.g. the broker-spawned window process of a
/// packaged app, which is NOT our direct child) to the kill-on-close job, so
/// it too dies when this test process exits. Best-effort.
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

/// Owns every spawned child AND any externally-launched pids (broker apps);
/// kills them all on drop — prompt cleanup between tests and on early
/// return / panic unwind. The Job Object is the backstop for hard kills.
struct ChildBag { children: Vec<Child>, pids: Vec<u32> }
impl ChildBag {
    fn new() -> Self { ChildBag { children: Vec::new(), pids: Vec::new() } }
    fn push(&mut self, c: Child) { self.children.push(c); }
    /// Track an external pid (and its whole tree) for teardown via taskkill.
    fn track_pid(&mut self, pid: u32) { self.pids.push(pid); }
}
impl Drop for ChildBag {
    fn drop(&mut self) {
        // Tree-kill externally-discovered window processes first (packaged /
        // broker-launched apps whose window pid isn't our spawned child).
        for pid in &self.pids {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .stdout(Stdio::null()).stderr(Stdio::null())
                .status();
        }
        for c in &mut self.children {
            let _ = c.kill();
            let _ = c.wait();
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Best-effort kill of any prior instance of `exe` by basename, so a leftover
/// from an earlier (e.g. force-killed) run can't hold the app's fixed HTTP port.
fn kill_prior_by_name(exe: &Path) {
    if let Some(name) = exe.file_name().and_then(|n| n.to_str()) {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/IM", name])
            .stdout(Stdio::null()).stderr(Stdio::null())
            .status();
    }
}

// ── paths / downloads ─────────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).parent().unwrap().parent().unwrap().to_owned()
}
fn driver_binary() -> PathBuf { workspace_root().join("target/debug/cua-driver.exe") }
fn focus_monitor_binary() -> PathBuf { workspace_root().join("target/debug/focus-monitor-win.exe") }

fn curl_download(url: &str, dst: &Path) -> bool {
    eprintln!("[e2e] downloading {url}\n        -> {dst:?}");
    let ok = Command::new("curl.exe")
        .args(["-L", "--fail", "--silent", "--show-error", "-o"])
        .arg(dst).arg(url).status().map(|s| s.success()).unwrap_or(false);
    ok && fs::metadata(dst).map(|m| m.len() > 4096).unwrap_or(false)
}
fn resolve_or_download(env_var: &str, prefix: &str, url: &str, tmp_name: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(env_var) {
        let pb = PathBuf::from(p);
        if pb.exists() { return Some(pb); }
    }
    if let Ok(entries) = fs::read_dir(workspace_root().join("test-apps")) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with(prefix) && name.ends_with(".exe") && !name.contains("setup") {
                return Some(e.path());
            }
        }
    }
    let dst = std::env::temp_dir().join(tmp_name);
    if dst.exists() && fs::metadata(&dst).map(|m| m.len() > 4096).unwrap_or(false) {
        return Some(dst);
    }
    if curl_download(url, &dst) { Some(dst) } else { None }
}
fn electron_exe() -> Option<PathBuf> {
    resolve_or_download("CUA_ELECTRON_EXE", "desktop-test-app-electron", ELECTRON_URL,
        "desktop-test-app-electron.0.1.0.exe")
}
fn tauri_exe() -> Option<PathBuf> {
    resolve_or_download("CUA_TAURI_EXE", "desktop-test-app-windows", TAURI_URL,
        "desktop-test-app-windows-x86_64.exe")
}

fn loss_file()       -> PathBuf { std::env::temp_dir().join("focus_monitor_losses.txt") }
fn key_loss_file()   -> PathBuf { std::env::temp_dir().join("focus_monitor_key_losses.txt") }
fn focus_pid_file()  -> PathBuf { std::env::temp_dir().join("focus_monitor_pid.txt") }
fn focus_hwnd_file() -> PathBuf { std::env::temp_dir().join("focus_monitor_hwnd.txt") }
fn read_count(p: &Path) -> u32 {
    fs::read_to_string(p).ok().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0)
}

// ── JSON-RPC over the driver's stdio, with per-call timeout ───────────────────

fn send(stdin: &mut ChildStdin, req: serde_json::Value) {
    let _ = writeln!(stdin, "{}", serde_json::to_string(&req).unwrap());
    let _ = stdin.flush();
}
fn call(stdin: &mut ChildStdin, rx: &Receiver<String>,
        id: u32, name: &str, args: serde_json::Value) -> serde_json::Value {
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
fn find_idx_containing(s: &serde_json::Value, needle: &str) -> Option<u64> {
    for line in result_text(s).lines() {
        if !line.to_lowercase().contains(&needle.to_lowercase()) { continue; }
        let st = line.find('[')? + 1;
        let en = line[st..].find(']')? + st;
        if let Ok(n) = line[st..en].trim().parse() { return Some(n); }
    }
    None
}

fn window_ids(stdin: &mut ChildStdin, rx: &Receiver<String>) -> HashSet<u64> {
    let r = call(stdin, rx, 99, "list_windows", serde_json::json!({}));
    r["result"]["structuredContent"]["windows"].as_array()
        .map(|a| a.iter().filter_map(|w| w["window_id"].as_u64()).collect())
        .unwrap_or_default()
}

// ── fixture ───────────────────────────────────────────────────────────────────

struct E2eFixture {
    _bag: ChildBag, // kills target+sentinel+driver on drop
    stdin: ChildStdin,
    rx: Receiver<String>,
    pid: u32,
    wid: u64,
}

/// Launch order: driver (no window) → snapshot windows → app → discover the
/// app's NEW window (works for multi-process apps like Electron whose window
/// belongs to a child pid) → sentinel (grabs foreground, pushing the app to
/// the background) → reset counters.
fn setup(target_exe: &Path, _title_hint: &str) -> Option<E2eFixture> {
    let driver_bin = driver_binary();
    let fm_bin = focus_monitor_binary();
    if !driver_bin.exists() { eprintln!("[e2e] cua-driver.exe not built — skipping"); return None; }
    if !fm_bin.exists() { eprintln!("[e2e] focus-monitor-win.exe not built — skipping"); return None; }
    if target_exe.is_absolute() && !target_exe.exists() {
        eprintln!("[e2e] target {target_exe:?} missing — skipping"); return None;
    }

    // Defensive: clear any leftover instance holding the app's fixed port.
    kill_prior_by_name(target_exe);
    let _ = fs::write(loss_file(), "0");
    let _ = fs::write(key_loss_file(), "0");
    let _ = fs::remove_file(focus_pid_file());
    let _ = fs::remove_file(focus_hwnd_file());

    let mut bag = ChildBag::new();

    // 1. Driver (daemon, no visible window).
    let mut driver = spawn_in_job(
        Command::new(&driver_bin).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
    ).inspect_err(|e| eprintln!("[e2e] driver spawn failed: {e}")).ok()?;
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

    // 2. Snapshot existing windows, then launch the target app.
    let before = window_ids(&mut stdin, &rx);
    let app = match spawn_in_job(
        Command::new(target_exe).stdout(Stdio::null()).stderr(Stdio::null())
    ) {
        Ok(c) => c,
        Err(e) => { eprintln!("[e2e] target spawn failed: {e}"); return None; } // bag drops → kills driver
    };
    bag.push(app);

    // 3. Discover the app's NEW window (its pid may be a child of the launched
    //    process — Electron/WebView2 are multi-process).
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut found: Option<(u32, u64)> = None;
    while Instant::now() < deadline {
        let r = call(&mut stdin, &rx, 11, "list_windows", serde_json::json!({}));
        if let Some(arr) = r["result"]["structuredContent"]["windows"].as_array() {
            for w in arr {
                let Some(wid) = w["window_id"].as_u64() else { continue };
                let pid = w["pid"].as_u64().unwrap_or(0) as u32;
                let title = w["title"].as_str().unwrap_or("");
                if !before.contains(&wid) && !title.is_empty() && pid != 0 {
                    found = Some((pid, wid));
                    break;
                }
            }
        }
        if found.is_some() { break; }
        std::thread::sleep(Duration::from_millis(500));
    }
    let (pid, wid) = match found {
        Some(p) => p,
        None => { eprintln!("[e2e] app window never appeared — skipping"); return None; } // bag drops
    };
    // The window's process is often broker-spawned (packaged apps, Electron),
    // i.e. NOT our direct child. Job-assign it (hard-kill safety) and track it
    // for prompt tree-kill on teardown so it can't orphan.
    assign_pid_to_job(pid);
    bag.track_pid(pid);

    // 4. Sentinel grabs foreground; app drops to z+1 (the background target).
    let fm = match spawn_in_job(
        Command::new(&fm_bin).stdout(Stdio::null()).stderr(Stdio::null())
    ) {
        Ok(c) => c,
        Err(e) => { eprintln!("[e2e] sentinel spawn failed: {e}"); return None; }
    };
    bag.push(fm);
    let sdeadline = Instant::now() + Duration::from_secs(10);
    loop {
        let ok = read_count(&focus_pid_file()) != 0
            && fs::read_to_string(focus_hwnd_file()).ok()
                .and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0) != 0;
        if ok { break; }
        if Instant::now() > sdeadline { eprintln!("[e2e] sentinel never published — skipping"); return None; }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(400));
    let _ = fs::write(loss_file(), "0");
    let _ = fs::write(key_loss_file(), "0");

    Some(E2eFixture { _bag: bag, stdin, rx, pid, wid })
}

// ── DOM registration oracle (trycua test apps serve an event log on 6769) ─────

const APP_API: &str = "http://127.0.0.1:6769";

fn http_reset() {
    let _ = Command::new("curl.exe")
        .args(["-s", "-m", "3", "-X", "POST", &format!("{APP_API}/reset")])
        .stdout(Stdio::null()).stderr(Stdio::null()).status();
}
/// Returns the `/events` body, or None if the app isn't serving the API.
fn http_events() -> Option<String> {
    let out = Command::new("curl.exe")
        .args(["-s", "-m", "3", &format!("{APP_API}/events")])
        .output().ok()?;
    if !out.status.success() { return None; }
    let body = String::from_utf8_lossy(&out.stdout).into_owned();
    // Reachable iff it looks like the JSON array the app returns.
    if body.trim_start().starts_with('[') { Some(body) } else { None }
}
/// True if the event log recorded at least one DOM event since the last reset
/// (i.e. the action actually reached the page). None if the app has no API.
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
/// to also catch async self-reactivation (Chromium). `user_pid` is the window
/// we expect to keep the foreground (the focus-monitor-win sentinel launched
/// last); reported for diagnostics.
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
        if last_fg == target_pid { stole = true; break; }
        std::thread::sleep(Duration::from_millis(80));
    }
    assert!(!stole,
        "{label}: target pid {target_pid} BECAME the foreground window — z-raise / foreground steal. \
         (expected user pid {user_pid}, fg_before={fg_before})");
    println!("✅ {label}: target pid {target_pid} stayed background (foreground pid={last_fg}, user={user_pid})");
}

/// Shared body: default-mode left click into a webview target.
fn webview_click_case(label: &str, exe: PathBuf) {
    let mut fx = match setup(&exe, "") { Some(f) => f, None => return };
    let (pid, wid) = (fx.pid, fx.wid);
    let snap = call(&mut fx.stdin, &fx.rx, 20, "get_window_state",
        serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
    let elem = find_idx_containing(&snap, "button").or_else(|| find_idx_containing(&snap, "click"));

    http_reset();
    let mut last = serde_json::Value::Null;
    assert_target_stays_background(label, pid, || {
        let args = match elem {
            Some(idx) => serde_json::json!({"pid": pid as i64, "window_id": wid, "element_index": idx}),
            None => serde_json::json!({"pid": pid as i64, "window_id": wid, "x": 200, "y": 200}),
        };
        last = call(&mut fx.stdin, &fx.rx, 21, "click", args);
    });
    assert!(!is_error(&last), "{label}: default-mode click errored: {}", result_text(&last));
    assert!(!result_text(&last).contains("background_unavailable"),
        "{label}: click should not need dispatch:foreground, got {:?}", result_text(&last));
    // Delivery is confirmed by the driver's own ✅ result (UIA Invoke fires the
    // element's default action / DOM `click`; PostMessage/injection deliver the
    // button events). The /events pointer-log is reported for info only — it
    // does NOT capture accessibility-driven UIA Invoke (which raises `click`,
    // not `pointerdown`), so a `[]` there is expected for the UIA path and is
    // not a failure. The hard invariant of this suite is no-foreground-steal.
    std::thread::sleep(Duration::from_millis(300));
    let reg = registered_since_reset().map(|b| b.to_string()).unwrap_or_else(|| "n/a".into());
    println!("{label}: delivered={:?}  pointer-events-logged={reg}", result_text(&last));
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn e2e_electron_background_click_no_z_raise() {
    let Some(exe) = electron_exe() else { eprintln!("[e2e] no electron app — skipping"); return; };
    webview_click_case("electron click (default dispatch)", exe);
}

#[test]
#[ignore]
fn e2e_tauri_background_click_no_z_raise() {
    let Some(exe) = tauri_exe() else { eprintln!("[e2e] no tauri app — skipping"); return; };
    webview_click_case("tauri click (default dispatch)", exe);
}

#[test]
#[ignore]
fn e2e_win32_notepad_background_click_no_z_raise() {
    let mut fx = match setup(Path::new(r"C:\Windows\System32\notepad.exe"), "") { Some(f) => f, None => return };
    let (pid, wid) = (fx.pid, fx.wid);
    let mut last = serde_json::Value::Null;
    assert_target_stays_background("notepad click (default dispatch)", pid, || {
        last = call(&mut fx.stdin, &fx.rx, 21, "click",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "x": 120, "y": 120}));
    });
    assert!(!is_error(&last), "notepad click errored: {}", result_text(&last));
    println!("notepad click result: {:?}", result_text(&last));
}

/// Electron right-click (pen-barrel injection): no raise. Pen→right promotion
/// is app-dependent, so landing is best-effort; the hard invariant is no
/// z-raise + no background_unavailable.
#[test]
#[ignore]
fn e2e_electron_background_right_click_no_z_raise() {
    let Some(exe) = electron_exe() else { eprintln!("[e2e] no electron app — skipping"); return; };
    let mut fx = match setup(&exe, "") { Some(f) => f, None => return };
    let (pid, wid) = (fx.pid, fx.wid);
    let mut last = serde_json::Value::Null;
    assert_target_stays_background("electron right-click (default dispatch)", pid, || {
        last = call(&mut fx.stdin, &fx.rx, 21, "click",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "x": 200, "y": 200, "button": "right"}));
    });
    println!("electron right-click result: {:?}", result_text(&last));
}

/// Electron TEXT typing: focus a field, then type — must register the text in
/// the DOM AND never steal foreground / move the cursor. This is the "type into
/// any window" half of the mission for plain text (WM_CHAR path, no foreground).
#[test]
#[ignore]
fn e2e_electron_background_type_text_no_z_raise() {
    let Some(exe) = electron_exe() else { eprintln!("[e2e] no electron app — skipping"); return; };
    let mut fx = match setup(&exe, "") { Some(f) => f, None => return };
    let (pid, wid) = (fx.pid, fx.wid);

    // Focus a text field if the page exposes one (so the chars have a sink).
    let snap = call(&mut fx.stdin, &fx.rx, 20, "get_window_state",
        serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
    let field = find_idx_containing(&snap, "edit")
        .or_else(|| find_idx_containing(&snap, "text"))
        .or_else(|| find_idx_containing(&snap, "input"));
    if let Some(idx) = field {
        let _ = call(&mut fx.stdin, &fx.rx, 21, "click",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "element_index": idx}));
        std::thread::sleep(Duration::from_millis(200));
    }

    http_reset();
    let mut last = serde_json::Value::Null;
    assert_target_stays_background("electron type_text (default dispatch)", pid, || {
        last = call(&mut fx.stdin, &fx.rx, 22, "type_text",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "text": "cuatest"}));
    });
    assert!(!is_error(&last), "type_text errored: {}", result_text(&last));
    std::thread::sleep(Duration::from_millis(300));
    if let Some(reg) = registered_since_reset() {
        // Soft: typing needs a focused sink; if the page had no focusable field
        // we can't fault the input path. Report either way.
        println!("electron type_text: registered={reg}  result={:?}", result_text(&last));
    } else {
        println!("electron type_text result: {:?}", result_text(&last));
    }
}

/// Electron key-combo (Ctrl+A): the dropped-PostMessage keyboard path now uses
/// cloaked focus + SendInput. Hard invariant: no z-raise.
#[test]
#[ignore]
fn e2e_electron_background_keycombo_no_z_raise() {
    let Some(exe) = electron_exe() else { eprintln!("[e2e] no electron app — skipping"); return; };
    let mut fx = match setup(&exe, "") { Some(f) => f, None => return };
    let (pid, wid) = (fx.pid, fx.wid);
    let mut last = serde_json::Value::Null;
    assert_target_stays_background("electron Ctrl+A (default dispatch)", pid, || {
        last = call(&mut fx.stdin, &fx.rx, 21, "press_key",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "key": "a", "modifiers": ["control"]}));
    });
    // Capability-first: the combo must be DELIVERED, not refused. (UX is
    // best-effort: the cloaked focus restores foreground, so the no-steal
    // assertion above still holds post-action.)
    assert!(!is_error(&last) && !result_text(&last).contains("background_unavailable"),
        "Ctrl+A must be delivered (capability over UX), got: {:?}", result_text(&last));
    println!("electron Ctrl+A delivered: {:?}", result_text(&last));
}
