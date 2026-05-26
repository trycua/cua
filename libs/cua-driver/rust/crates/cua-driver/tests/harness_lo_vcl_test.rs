//! Integration tests against a live LibreOffice Writer instance.
//!
//! Regression-guards the VCL/SAL gaps the vision-only LO Writer flow
//! surfaced (manually validated during the PR #1708 follow-up
//! exploration). Each test asserts the *current* gap — it will fail
//! loudly if cua-driver later closes the underlying gap, prompting
//! a flip of the assertion.
//!
//! ## Documented gap under regression guard
//!
//! **VCL toolbar SplitButtons expose only InvokePattern, no
//! ExpandCollapse.** The "Font Color" button in the Formatting
//! toolbar is a SplitButton (apply-current-color on the left half,
//! open-color-picker on the right). UIA reports it as a single
//! element with `actions=[invoke]` — no separate child for the
//! dropdown arrow, no ExpandCollapsePattern. Pixel-clicking either
//! half resolves through cua-driver's `try_invoke_in_window_at_point`
//! fallback to the parent's `Invoke()`, which applies the current
//! color — there is **no way to open the color picker** through the
//! toolbar SplitButton at all. The agent has to go through Format →
//! Character instead.
//!
//! ## Gaps explored that turned out NOT to reproduce
//!
//! During the exploration we suspected VCL modal dialogs (SALSUBFRAME
//! class) might silently drop SendInput-injected keystrokes. A
//! repro attempt against Find & Replace (Ctrl+H) showed Escape
//! foreground-dispatch closes the dialog cleanly. The earlier
//! Character-dialog observation that motivated the suspicion was
//! likely confounded by the SplitButton miscalc + general flake; it
//! is not stable enough to guard against here.
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

// ── Gap #1: FontColor SplitButton exposes only InvokePattern ─────────────────

/// Documents that the LO Writer "Font Color" toolbar SplitButton has no
/// ExpandCollapse pattern exposed in the UIA tree — only `Invoke`. So
/// the agent can apply the current color but can't programmatically
/// open the color picker through the toolbar at all (has to go via
/// Format → Character).
///
/// If this assertion flips (the SplitButton now reports `expand`), the
/// underlying VCL SalToolBox.AddElement code path was updated to expose
/// a child for the dropdown arrow — at which point flip this test to
/// the positive assertion and a follow-up test can drive picker-open
/// → pick-color.
#[test]
#[ignore]
fn harness_lo_vcl_font_color_split_button_DOCUMENTED_no_expand() {
    let mut fx = match setup() { Some(s) => s, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        let snap = call(stdin, stdout, 30, "get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "capture_mode": "ax", "query": "Font Color"
        }));
        let text = snapshot_text(&snap);
        assert!(text.contains("SplitButton \"Font Color\""),
            "Font Color SplitButton not found in UIA tree: {text:?}");
        // Inspect the actions=[...] list on the Font Color line. The
        // documented gap is that ONLY `invoke` is reported — no
        // `expand`. When VCL exposes the dropdown child, `expand` will
        // appear.
        let line = text.lines().find(|l| l.contains("Font Color")).unwrap_or("");
        assert!(line.contains("actions=[invoke]"),
            "Expected `actions=[invoke]` ONLY on the Font Color SplitButton — \
             got {line:?}. If `expand` is in the list now, cua-driver may have \
             gained ExpandCollapse on VCL SplitButtons; flip the assertion.");
        assert!(!line.contains("expand"),
            "Font Color SplitButton now reports ExpandCollapse — VCL gap closed?");
        println!("⚠️  harness_lo_vcl_font_color_split_button_DOCUMENTED_no_expand: \
                  Font Color SplitButton actions=[invoke] only (no expand) — \
                  no programmatic way to open the color picker via the toolbar.");
    });
}

// ── Working path: SALSUBFRAME modal dialogs accept SendInput input ─────────

/// Confirms that VCL/SAL modal dialogs (SALSUBFRAME class) DO accept
/// SendInput-injected keyboard input, despite an earlier hypothesis to
/// the contrary that came up during the vision-only LO exploration.
///
/// Specifically:
///   - `hotkey(ctrl+h, foreground)` against the main Writer SALFRAME
///     opens the Find & Replace dialog (a SALSUBFRAME).
///   - The dialog snapshot via `get_window_state(capture_mode:"ax")`
///     returns the documented SAL-skip stub (UIA walk would hang on
///     `TreeScope.Subtree` from the daemon's MTA pool, so cua-driver
///     short-circuits — `uia/mod.rs`).
///   - `press_key(escape, foreground)` against the SALSUBFRAME closes
///     it cleanly.
///
/// If this test ever fails, one of three things regressed:
///   (a) Ctrl+H accelerator on the main Writer window stopped firing
///       under SendInput (foreground key dispatch broken on SALFRAME).
///   (b) The SAL-skip stub message changed wording (`uia/mod.rs`).
///   (c) Foreground Escape stopped reaching the SALSUBFRAME (SAL filter
///       changed in a newer LO).
///
/// Background dispatch is intentionally NOT tested here for the open
/// step — cua-driver explicitly rejects `dispatch:"background"` for
/// `key_combo` on SALFRAME with a structured error (see
/// `platform-windows/src/tools/impl_.rs`). Probing it would just
/// confirm that error, which the unit tests already cover.
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

        // Snapshot the dialog — should return the SAL-skip stub.
        let dialog_ax = call(stdin, stdout, 32, "get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": dialog_wid, "capture_mode": "ax"
        }));
        let dialog_text = snapshot_text(&dialog_ax);
        assert!(dialog_text.contains("SAL/VCL target, UIA walk skipped"),
            "Expected SAL-skip stub for SALSUBFRAME dialog, got: {dialog_text:?}. \
             If this assertion flips, cua-driver may have a working UIA walk on \
             SAL dialogs now — celebrate and update the test.");

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
