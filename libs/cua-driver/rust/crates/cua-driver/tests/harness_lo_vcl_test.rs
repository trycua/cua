//! Integration tests against a live LibreOffice Writer instance.
//!
//! Regression-guards the VCL/SAL gaps the vision-only LO Writer flow
//! surfaced (manually validated during the PR #1708 follow-up
//! exploration). Each test asserts the *current* gap — it will fail
//! loudly if cua-driver later closes the underlying gap, prompting
//! a flip of the assertion.
//!
//! ## Tests in this file
//!
//! 1. **`harness_lo_vcl_font_color_split_button_exposes_expand`** —
//!    positive guard for the MSAA fallback (see `platform-windows/src/msaa.rs`,
//!    landed alongside this file). Asserts the toolbar "Font Color"
//!    SplitButton now reports `actions=[invoke,expand]` via the MSAA
//!    walker (UIA's MSAA→UIA proxy collapsed it to bare `[invoke]`
//!    before — guarded by an inverted assertion until cua-driver
//!    gained the MSAA path).
//!
//! 2. **`harness_lo_vcl_font_color_expand_opens_picker`** — end-to-end
//!    guard for the `action:"expand"` dispatch. Calls
//!    `click(element_index=<Font Color>, action:"expand")` and
//!    asserts a new top-level `SALTMPSUBFRAME` titled "Font Color"
//!    appears (the picker popup). This is the workflow the MSAA
//!    fallback was built to unlock.
//!
//! 3. **`harness_lo_vcl_modal_input_roundtrip_works`** — confirms
//!    SAL/VCL modal dialogs (SALSUBFRAME class) DO accept
//!    SendInput-injected input. Opens Find & Replace via Ctrl+H,
//!    closes via Escape. Catches regressions in SALFRAME accelerator
//!    dispatch and SALSUBFRAME key handling.
//!
//! ## How to run
//!
//! Local (requires LibreOffice installed):
//!   cargo test --test harness_lo_vcl_test -- --ignored --nocapture
//!
//! Tests skip cleanly if `swriter.exe` isn't on disk at one of the
//! standard install locations (override via `LO_SWRITER_EXE`).

#![cfg(target_os = "windows")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

// ── workspace / LO paths ─────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).parent().unwrap().parent().unwrap().to_owned()
}
fn driver_binary() -> PathBuf { workspace_root().join("target/debug/cua-driver.exe") }

/// LO Writer executable. Honour `LO_SWRITER_EXE` env override (for CI
/// images with non-default install paths), otherwise probe the two
/// standard locations.
fn swriter_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LO_SWRITER_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() { return Some(pb); }
    }
    for candidate in [
        r"C:\Program Files\LibreOffice\program\swriter.exe",
        r"C:\Program Files (x86)\LibreOffice\program\swriter.exe",
    ] {
        let pb = PathBuf::from(candidate);
        if pb.exists() { return Some(pb); }
    }
    None
}

// ── JSON-RPC plumbing ────────────────────────────────────────────────────────

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

// ── fixture ──────────────────────────────────────────────────────────────────

struct LoSession {
    _writer: Child,
    driver: Child,
    driver_stdin: ChildStdin,
    driver_stdout: ChildStdout,
    writer_pid: u32,
    writer_wid: u64,
}

impl Drop for LoSession {
    fn drop(&mut self) {
        // Try clean shutdown via cua-driver kill_app first so LO doesn't
        // leak a soffice.bin background daemon across test runs.
        let _ = self.driver.kill(); let _ = self.driver.wait();
        let _ = self._writer.kill(); let _ = self._writer.wait();
        // SAL/VCL soffice.bin daemon hangs around after the parent exits;
        // best-effort sweep via taskkill so the next test starts clean.
        let _ = Command::new("taskkill").args(["/F", "/IM", "soffice.bin"]).output();
        std::thread::sleep(Duration::from_millis(800));
    }
}

fn setup() -> Option<LoSession> {
    let driver = driver_binary();
    if !driver.exists() {
        eprintln!("cua-driver.exe not built — run `cargo build` first"); return None;
    }
    let writer = match swriter_exe() {
        Some(p) => p,
        None => {
            eprintln!("LibreOffice swriter.exe not found — set LO_SWRITER_EXE or install LO");
            return None;
        }
    };

    // Sweep any leftover soffice.bin from a prior run so this test gets
    // a fresh writer pid + clean recovery state.
    let _ = Command::new("taskkill").args(["/F", "/IM", "soffice.bin"]).output();
    std::thread::sleep(Duration::from_millis(800));

    // `-norestore` skips the Document Recovery dialog (which would
    // block the Writer top-level window from ever appearing if the
    // previous LO session crashed). `-nologo` skips the splash screen.
    let _writer = Command::new(&writer)
        .args(["-norestore", "-nologo"])
        .stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().ok()?;
    // LO launches via soffice.bin background daemon, then forks the
    // actual writer process. The pid we just spawned is the launcher;
    // the real writer is a child. Poll list_apps via cua-driver to
    // find it.
    std::thread::sleep(Duration::from_secs(3));

    let mut driver_c = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().ok()?;
    let mut driver_stdin = driver_c.stdin.take().unwrap();
    let mut driver_stdout = driver_c.stdout.take().unwrap();
    {
        let mut stdout = BufReader::new(&mut driver_stdout);
        init(&mut driver_stdin, &mut stdout);
        // Find the LO Writer window by title — its owning pid is the
        // soffice.bin daemon (NOT the launcher pid above). Poll up to
        // 30 s for the recovery dialog (if any) + main window to appear.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let resp = call(&mut driver_stdin, &mut stdout, 10, "list_windows", serde_json::json!({}));
            if let Some(wins) = resp["result"]["structuredContent"]["windows"].as_array() {
                for w in wins {
                    let title = w["title"].as_str().unwrap_or("");
                    let app = w["app_name"].as_str().unwrap_or("");
                    if app.contains("soffice") && title.contains("LibreOffice Writer") {
                        let pid = w["pid"].as_u64().unwrap_or(0) as u32;
                        let wid = w["window_id"].as_u64().unwrap_or(0);
                        if pid > 0 && wid > 0 {
                            drop(stdout);
                            return Some(LoSession {
                                _writer, driver: driver_c, driver_stdin, driver_stdout,
                                writer_pid: pid, writer_wid: wid,
                            });
                        }
                    }
                }
            }
            if std::time::Instant::now() > deadline {
                eprintln!("Writer window did not appear within 30 s");
                drop(stdout);
                let _ = driver_c.kill();
                return None;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

fn with_session<F, R>(fx: &mut LoSession, f: F) -> R
where F: FnOnce(&mut ChildStdin, &mut BufReader<&mut ChildStdout>, u32, u64) -> R {
    let mut stdout = BufReader::new(&mut fx.driver_stdout);
    f(&mut fx.driver_stdin, &mut stdout, fx.writer_pid, fx.writer_wid)
}

// ── Test 1: Font Color SplitButton exposes `expand` via MSAA fallback ───────

/// Confirms the MSAA fallback (`platform-windows/src/msaa.rs`) lets
/// cua-driver see VCL toolbar SplitButtons' dropdown halves.
///
/// Before the MSAA fallback landed: UIA's MSAA→UIA proxy collapsed
/// `ROLE_SYSTEM_BUTTONDROPDOWN` (0x38) to a featureless `SplitButton`
/// with `actions=[invoke]` — agents could re-fire the press half but
/// not open the picker. After the MSAA fallback: SAL-class windows
/// walk via oleacc's `AccessibleObjectFromWindow`, which preserves
/// the BUTTONDROPDOWN role, and cua-driver maps it to
/// `actions=[invoke,expand]`. The `click` tool's `action:"expand"`
/// case clicks the right-edge of the cached rect (the dropdown arrow
/// half) via SendInput.
///
/// If this fails: either the MSAA path stopped applying to SALFRAME
/// (check `uia/mod.rs` SAL class detection), or LO changed its
/// toolbar role (no longer ROLE_SYSTEM_BUTTONDROPDOWN).
#[test]
#[ignore]
fn harness_lo_vcl_font_color_split_button_exposes_expand() {
    let mut fx = match setup() { Some(s) => s, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        let snap = call(stdin, stdout, 30, "get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "capture_mode": "ax", "query": "Font Color"
        }));
        let text = snapshot_text(&snap);
        assert!(text.contains("SplitButton \"Font Color\""),
            "Font Color SplitButton not found in tree: {text:?}");
        let line = text.lines().find(|l| l.contains("\"Font Color\"")).unwrap_or("");
        assert!(line.contains("expand"),
            "Expected `expand` in actions on Font Color SplitButton — got {line:?}. \
             The MSAA fallback may not be running for SALFRAME (check uia/mod.rs \
             SAL class detection — should route ALL SAL* classes through msaa.rs).");
        assert!(line.contains("invoke"),
            "Expected `invoke` to remain in actions alongside `expand` — got {line:?}. \
             MSAA walker should expose both press and dropdown halves.");
    });
}

// ── Test 2: action:"expand" actually opens the color picker ─────────────────

/// End-to-end test that the `click(element_index, action:"expand")`
/// dispatch on a MSAA BUTTONDROPDOWN actually opens the dropdown.
///
/// Asserts:
///   1. `get_window_state` finds Font Color by name and yields an
///      element_index.
///   2. `click(element_index=X, action:"expand")` returns success.
///   3. A new top-level window appears under the LO pid with title
///      "Font Color" (the SALTMPSUBFRAME color picker).
///
/// Failures point at:
///   - (1) MSAA walker not finding Font Color (check msaa.rs walker
///         budget / depth, or LO renamed the button).
///   - (2) cua-driver click tool's MSAA dispatch broke (check
///         `tools/impl_.rs` BUTTONDROPDOWN branch).
///   - (3) Right-edge offset wrong for current LO version's toolbar
///         scale (check `rect.right - 4` heuristic in
///         `tools/impl_.rs`).
#[test]
#[ignore]
fn harness_lo_vcl_font_color_expand_opens_picker() {
    let mut fx = match setup() { Some(s) => s, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        // Bring Writer to foreground so SendInput click lands on it.
        let _ = call(stdin, stdout, 30, "bring_to_front", serde_json::json!({
            "pid": pid as i64, "window_id": wid
        }));
        std::thread::sleep(Duration::from_millis(400));

        let snap = call(stdin, stdout, 31, "get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "capture_mode": "ax", "query": "Font Color"
        }));
        let text = snapshot_text(&snap);
        let line = text.lines()
            .find(|l| l.contains("\"Font Color\"") && l.contains("expand"))
            .unwrap_or_else(|| panic!("Font Color SplitButton with `expand` action not found: {text:?}"));
        let s = line.find('[').expect("element_index bracket open");
        let e = line[s..].find(']').expect("element_index bracket close") + s;
        let idx: u64 = line[s+1..e].trim().parse()
            .unwrap_or_else(|_| panic!("could not parse element_index from line {line:?}"));

        // Snapshot windows under our pid BEFORE the click.
        let before = call(stdin, stdout, 32, "list_windows", serde_json::json!({
            "pid": pid as i64
        }));
        let before_ids: std::collections::HashSet<u64> = before
            ["result"]["structuredContent"]["windows"].as_array()
            .map(|a| a.iter().filter_map(|w| w["window_id"].as_u64()).collect())
            .unwrap_or_default();

        let resp = call(stdin, stdout, 33, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "element_index": idx, "action": "expand"
        }));
        let resp_text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(resp_text.starts_with("✅"),
            "click(action:expand) failed: {resp_text:?}");
        assert!(resp_text.contains("dropdown half"),
            "Expected response to mention dropdown half — got {resp_text:?}. \
             The MSAA dispatch path may not have triggered.");

        // Let the picker spawn.
        std::thread::sleep(Duration::from_millis(900));

        let after = call(stdin, stdout, 34, "list_windows", serde_json::json!({
            "pid": pid as i64
        }));
        let new_wins: Vec<&serde_json::Value> = after
            ["result"]["structuredContent"]["windows"].as_array()
            .map(|a| a.iter()
                .filter(|w| {
                    let id = w["window_id"].as_u64();
                    id.map(|i| !before_ids.contains(&i)).unwrap_or(false)
                })
                .collect())
            .unwrap_or_default();
        let picker = new_wins.iter().find(|w| {
            w["title"].as_str().map(|t| t.contains("Font Color")).unwrap_or(false)
        });
        assert!(picker.is_some(),
            "No new window titled 'Font Color' appeared after click(action:expand). \
             new_windows={new_wins:?}. The right-edge dispatch may have hit the \
             wrong pixel (LO's toolbar scaled differently?) or the picker spawned \
             as a child window instead of a top-level.");
    });
}

// ── Working path: SALSUBFRAME modal dialogs accept SendInput input ─────────

/// Confirms the MSAA fallback gives a walkable tree on SALSUBFRAME
/// modal dialogs too (was the "SAL/VCL target, UIA walk skipped" stub
/// before the MSAA path landed), AND that the dialog accepts
/// SendInput-injected keyboard input.
///
/// Specifically:
///   - `hotkey(ctrl+h, foreground)` against the main Writer SALFRAME
///     opens the Find & Replace dialog (a SALSUBFRAME).
///   - `get_window_state(capture_mode:"ax")` on the dialog returns a
///     full element tree via the MSAA walker — includes the "Close"
///     button and the Find/Replace edit fields as addressable
///     element_indices (a recent capability — the pre-MSAA stub had
///     zero actionable elements).
///   - `press_key(escape, foreground)` against the SALSUBFRAME closes
///     it cleanly.
///
/// If this test ever fails, one of three things regressed:
///   (a) Ctrl+H accelerator on the main Writer window stopped firing
///       under SendInput (foreground key dispatch broken on SALFRAME).
///   (b) MSAA walker stopped finding actionable elements in
///       SALSUBFRAME (check `msaa.rs` budget / depth).
///   (c) Foreground Escape stopped reaching the SALSUBFRAME (SAL
///       filter changed in a newer LO).
#[test]
#[ignore]
fn harness_lo_vcl_modal_input_roundtrip_works() {
    let mut fx = match setup() { Some(s) => s, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        std::thread::sleep(Duration::from_millis(800));

        let open_resp = call(stdin, stdout, 30, "hotkey", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "keys": ["ctrl", "h"], "dispatch": "foreground"
        }));
        let open_text = open_resp["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(open_text.starts_with("✅"),
            "hotkey(ctrl+h, foreground) failed: {open_text:?}");

        let dialog_wid = {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            loop {
                let wins = call(stdin, stdout, 31, "list_windows", serde_json::json!({
                    "pid": pid as i64
                }));
                let found = wins["result"]["structuredContent"]["windows"].as_array()
                    .and_then(|a| a.iter().find_map(|w| {
                        let title = w["title"].as_str().unwrap_or("");
                        let id = w["window_id"].as_u64();
                        if id != Some(wid)
                           && (title.contains("Find") || title.contains("Replace"))
                        { id } else { None }
                    }));
                if let Some(id) = found { break id; }
                if std::time::Instant::now() > deadline {
                    panic!("Find & Replace dialog did not appear after Ctrl+H — \
                            Writer accelerator may have regressed under \
                            foreground SendInput on SALFRAME.");
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        };

        // Snapshot the dialog — MSAA walker should produce a real tree
        // with the dialog's buttons and edit fields, NOT the old skip
        // stub.
        let dialog_ax = call(stdin, stdout, 32, "get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": dialog_wid, "capture_mode": "ax"
        }));
        let dialog_text = snapshot_text(&dialog_ax);
        assert!(!dialog_text.contains("SAL/VCL target, UIA walk skipped"),
            "Got the old skip-stub message — MSAA fallback did not engage on \
             this SALSUBFRAME. Snapshot was: {dialog_text:?}");
        assert!(dialog_text.contains("Button \"Close\""),
            "Expected MSAA walker to expose a 'Close' Button in the Find & Replace \
             dialog tree (it's a documented child via accChild). Snapshot was: {dialog_text:?}");
        // Sanity: should have several actionable element_indices, not zero.
        let actionable_count = dialog_text.lines()
            .filter(|l| l.contains("actions=[invoke"))
            .count();
        assert!(actionable_count >= 4,
            "Expected ≥4 actionable elements in Find & Replace via MSAA walker, \
             got {actionable_count}. Tree: {dialog_text:?}");

        // Foreground Escape should close the dialog.
        let esc_resp = call(stdin, stdout, 33, "press_key", serde_json::json!({
            "pid": pid as i64, "window_id": dialog_wid,
            "key": "escape", "dispatch": "foreground"
        }));
        let esc_text = esc_resp["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(esc_text.starts_with("✅"),
            "press_key(escape, foreground) returned an error: {esc_text:?}");
        std::thread::sleep(Duration::from_millis(700));

        let wins_after = call(stdin, stdout, 34, "list_windows", serde_json::json!({
            "pid": pid as i64
        }));
        let still_open = wins_after["result"]["structuredContent"]["windows"].as_array()
            .map(|a| a.iter().any(|w| w["window_id"].as_u64() == Some(dialog_wid)))
            .unwrap_or(false);
        assert!(!still_open,
            "Find & Replace dialog stayed open after foreground Escape — \
             SAL modal input dispatch regressed. Check uia/mod.rs SAL-class \
             handling and the press_key SendInput path.");

        // Cleanup: kill the LO process — Drop impl sweeps soffice.bin.
        let _ = call(stdin, stdout, 35, "kill_app", serde_json::json!({
            "pid": pid as i64
        }));
    });
}
