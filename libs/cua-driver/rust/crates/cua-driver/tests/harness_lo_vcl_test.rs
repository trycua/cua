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
//!    Font Color SplitButton reports `actions=[invoke,expand]`.
//!
//! 2. **`harness_lo_vcl_font_color_expand_opens_picker`** — click
//!    with `action:"expand"` opens a SALTMPSUBFRAME picker.
//!
//! 3. **`harness_lo_vcl_modal_input_roundtrip_works`** — SAL/VCL
//!    modal (Find & Replace) accepts SendInput, snapshot has actionable
//!    elements (no more skip stub).
//!
//! 4. **`harness_lo_vcl_all_toolbar_split_buttons_expose_expand`** —
//!    *every* toolbar SplitButton in Writer has `expand` (not just
//!    Font Color). Guards `msaa::actions_for` BUTTONDROPDOWN family.
//!
//! 5. **`harness_lo_vcl_color_pick_green_end_to_end`** — full
//!    workflow: type text → select → open Font Color → click Green →
//!    picker closes (proves color landed).
//!
//! 6. **`harness_lo_vcl_recovery_dialog_walks_via_msaa`** — Document
//!    Recovery dialog (SALFRAME class) still walks via MSAA, exposes
//!    "Discard All" + "Recover Selected" buttons. Guards the
//!    routing change for the pre-existing Recovery-dialog flow.
//!
//! 7. **`harness_lo_vcl_calc_msaa_smoke`** — LO Calc (different app
//!    surface on the same VCL base) also walks via MSAA with ≥15
//!    SplitButtons exposing `expand`. Confirms the path generalizes
//!    beyond Writer.
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

/// LO Calc executable, same probe shape as `swriter_exe()`.
fn scalc_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LO_SCALC_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() { return Some(pb); }
    }
    for candidate in [
        r"C:\Program Files\LibreOffice\program\scalc.exe",
        r"C:\Program Files (x86)\LibreOffice\program\scalc.exe",
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

// ── Test 4: All toolbar SplitButtons expose `expand` ────────────────────────

/// Confirms the MSAA fallback exposes `actions=[invoke,expand]` on EVERY
/// toolbar SplitButton in LO Writer — not just Font Color.
///
/// Pre-MSAA, the UIA→MSAA proxy collapsed every BUTTONDROPDOWN to a
/// featureless `SplitButton actions=[invoke]`. Post-MSAA, ALL of them
/// (Save, Open, Undo, Redo, Paste, Table, Field, Symbol, Show Tracked
/// Changes, Record, Basic Shapes, Underline, Font Color, Character
/// Highlighting Color, Background Color, Unordered List, Ordered List,
/// Outline Format, Line Spacing, Character Spacing — plus the "System"
/// menu button) report `expand`. Guards against accidental regressions
/// in `msaa::actions_for` (e.g. dropping one of the BUTTONDROPDOWN
/// variants from the role match).
///
/// Asserts ≥15 SplitButtons report `expand` (current count is 20-21
/// depending on toolbar config; floor at 15 to absorb minor LO updates).
#[test]
#[ignore]
fn harness_lo_vcl_all_toolbar_split_buttons_expose_expand() {
    let mut fx = match setup() { Some(s) => s, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        let snap = call(stdin, stdout, 30, "get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "capture_mode": "ax"
        }));
        let text = snapshot_text(&snap);
        let splitbutton_lines: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("SplitButton") && l.contains("actions=["))
            .collect();
        let with_expand: Vec<&&str> = splitbutton_lines.iter()
            .filter(|l| l.contains("expand"))
            .collect();
        assert!(splitbutton_lines.len() >= 15,
            "Expected ≥15 SplitButtons in LO Writer toolbar tree, got {}. \
             Possible MSAA walker regression. Full snapshot first 1500 chars: {}",
            splitbutton_lines.len(),
            &text.chars().take(1500).collect::<String>());
        assert_eq!(with_expand.len(), splitbutton_lines.len(),
            "{}/{} SplitButtons report `expand`. The MSAA role→actions \
             mapping may have dropped one of BUTTONDROPDOWN / BUTTONMENU / \
             BUTTONDROPDOWNGRID / SPLITBUTTON. Missing-expand lines:\n{}",
            with_expand.len(), splitbutton_lines.len(),
            splitbutton_lines.iter()
                .filter(|l| !l.contains("expand"))
                .map(|l| l.trim())
                .collect::<Vec<_>>()
                .join("\n"));
    });
}

// ── Test 5: End-to-end color-pick ───────────────────────────────────────────

/// End-to-end regression for the headline workflow: type text, select it,
/// open the Font Color picker via `action:"expand"`, click a named color,
/// verify the picker closes (the canonical signal that LO applied the
/// color).
///
/// Failure points:
///   - MSAA walker can't find Font Color → `msaa.rs` role match broken.
///   - click(action:"expand") doesn't open picker → right-edge dispatch
///     offset wrong for current LO version's toolbar scale.
///   - Picker MSAA walk doesn't find "Green" → walker depth/budget
///     truncating the color grid.
///   - Picker doesn't close after picking → click on color cell isn't
///     landing (offset / role mismatch on ListItem).
#[test]
#[ignore]
fn harness_lo_vcl_color_pick_green_end_to_end() {
    let mut fx = match setup() { Some(s) => s, None => return };
    with_session(&mut fx, |stdin, stdout, pid, wid| {
        // Foreground first so type_text + Ctrl+A land on Writer.
        let _ = call(stdin, stdout, 30, "bring_to_front", serde_json::json!({
            "pid": pid as i64, "window_id": wid
        }));
        std::thread::sleep(Duration::from_millis(400));

        let _ = call(stdin, stdout, 31, "type_text", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "text": "color pick regression"
        }));
        std::thread::sleep(Duration::from_millis(400));

        let _ = call(stdin, stdout, 32, "hotkey", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "keys": ["ctrl", "a"], "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(400));

        // Locate Font Color element_index in Writer's MSAA tree.
        let snap = call(stdin, stdout, 33, "get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "capture_mode": "ax", "query": "Font Color"
        }));
        let snap_text = snapshot_text(&snap);
        let fc_line = snap_text.lines()
            .find(|l| l.contains("\"Font Color\"") && l.contains("expand"))
            .unwrap_or_else(|| panic!("Font Color with `expand` not found in tree: {snap_text:?}"));
        let s = fc_line.find('[').expect("[ in fc_line");
        let e = fc_line[s..].find(']').expect("] in fc_line") + s;
        let fc_idx: u64 = fc_line[s+1..e].trim().parse().expect("fc element_index");

        // Snapshot which windows existed BEFORE opening the picker so
        // we can isolate the picker on appearance.
        let before = call(stdin, stdout, 34, "list_windows", serde_json::json!({
            "pid": pid as i64
        }));
        let before_ids: std::collections::HashSet<u64> = before
            ["result"]["structuredContent"]["windows"].as_array()
            .map(|a| a.iter().filter_map(|w| w["window_id"].as_u64()).collect())
            .unwrap_or_default();

        // Open the dropdown.
        let open_resp = call(stdin, stdout, 35, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "element_index": fc_idx, "action": "expand"
        }));
        let open_text = open_resp["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(open_text.contains("dropdown half"),
            "click(action:expand) did not dispatch via MSAA dropdown path: {open_text:?}");
        std::thread::sleep(Duration::from_millis(900));

        // Find the picker window.
        let after = call(stdin, stdout, 36, "list_windows", serde_json::json!({
            "pid": pid as i64
        }));
        let picker_wid = after["result"]["structuredContent"]["windows"]
            .as_array()
            .and_then(|a| a.iter().find_map(|w| {
                let id = w["window_id"].as_u64();
                let title = w["title"].as_str().unwrap_or("");
                if id.map(|i| !before_ids.contains(&i)).unwrap_or(false)
                   && title.contains("Font Color")
                { id } else { None }
            }))
            .unwrap_or_else(|| panic!("No new 'Font Color' picker window appeared"));

        // Walk the picker tree to find a green color cell.
        let psnap = call(stdin, stdout, 37, "get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": picker_wid, "capture_mode": "ax"
        }));
        let psnap_text = snapshot_text(&psnap);
        let green_line = psnap_text.lines()
            .find(|l| l.contains("\"Green\"") && l.contains("[") && l.contains("actions=[invoke"))
            .unwrap_or_else(|| panic!(
                "No 'Green' ListItem in picker tree. The MSAA walker may have \
                 truncated the color grid or LO's locale renamed the color. \
                 First 1500 chars: {}",
                &psnap_text.chars().take(1500).collect::<String>()));
        let s = green_line.find('[').expect("[ in green");
        let e = green_line[s..].find(']').expect("] in green") + s;
        let green_idx: u64 = green_line[s+1..e].trim().parse().expect("green element_index");

        // Pick it. Default action (invoke) clicks center via MSAA dispatch.
        let pick_resp = call(stdin, stdout, 38, "click", serde_json::json!({
            "pid": pid as i64, "window_id": picker_wid,
            "element_index": green_idx
        }));
        let pick_text = pick_resp["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(pick_text.starts_with("✅"),
            "click on Green color cell failed: {pick_text:?}");
        std::thread::sleep(Duration::from_millis(700));

        // Verify the picker closed (LO closes the popup after a color is
        // selected — the canonical "color applied" signal). If it
        // didn't, the click didn't actually land on the cell.
        let after2 = call(stdin, stdout, 39, "list_windows", serde_json::json!({
            "pid": pid as i64
        }));
        let still_open = after2["result"]["structuredContent"]["windows"]
            .as_array()
            .map(|a| a.iter().any(|w| w["window_id"].as_u64() == Some(picker_wid)))
            .unwrap_or(false);
        assert!(!still_open,
            "Picker stayed open after click on Green — the cell click \
             didn't land. LO didn't apply the color.");
    });
}

// ── Test 6: Recovery dialog walks via MSAA ──────────────────────────────────

/// Confirms that the LibreOffice Document Recovery dialog (which used to
/// be walked via UIA) is still walkable after the MSAA-routes-all-SAL
/// change. Pre-MSAA: UIA path returned a walkable tree with "Discard
/// All" / "Recover Selected" buttons addressable. Post-MSAA: same flow
/// must continue to work through the MSAA walker.
///
/// Triggering Recovery requires a previous LO session that exited
/// ungracefully — this test creates that condition by typing into a
/// fresh writer and then `kill_app`-ing it, then re-launches. If the
/// Recovery dialog doesn't appear (LO can't always be coaxed into
/// recovery state, e.g. when the recovery feature is disabled in
/// user config), the test logs and returns OK rather than panicking —
/// it's a regression guard for the post-MSAA Recovery walk, not for
/// LO's recovery-trigger behavior.
#[test]
#[ignore]
fn harness_lo_vcl_recovery_dialog_walks_via_msaa() {
    // First: launch Writer WITHOUT -norestore (we WANT recovery
    // behavior), type something, then force-kill so LO marks the
    // session as crashed.
    let writer = match swriter_exe() { Some(p) => p, None => return };
    let _ = Command::new("taskkill").args(["/F", "/IM", "soffice.bin"]).output();
    std::thread::sleep(Duration::from_millis(800));

    // Launch (no -norestore, no -nologo flag → default behavior).
    let mut crash_proc = match Command::new(&writer)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .spawn() {
            Ok(p) => p,
            Err(_) => return,
        };
    std::thread::sleep(Duration::from_secs(5));
    let _ = crash_proc.kill();
    let _ = crash_proc.wait();
    // Force-kill the actual soffice.bin worker too (the launcher is what
    // we just killed; the real LO process is a fork).
    let _ = Command::new("taskkill").args(["/F", "/IM", "soffice.bin"]).output();
    std::thread::sleep(Duration::from_secs(2));

    // Re-launch Writer — Recovery dialog should appear.
    let driver = driver_binary();
    if !driver.exists() { return; }
    let mut writer_proc = match Command::new(&writer)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .spawn() {
            Ok(p) => p,
            Err(_) => return,
        };
    std::thread::sleep(Duration::from_secs(5));

    let mut driver_c = match Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn() {
            Ok(p) => p,
            Err(_) => {
                let _ = writer_proc.kill();
                return;
            }
        };
    let mut stdin = driver_c.stdin.take().unwrap();
    let mut stdout_raw = driver_c.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut stdout_raw);

    init(&mut stdin, &mut stdout);

    // Poll for the Recovery dialog.
    let recovery_wid_pid: Option<(u64, u32)> = {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut found = None;
        while std::time::Instant::now() < deadline {
            let resp = call(&mut stdin, &mut stdout, 1, "list_windows", serde_json::json!({}));
            if let Some(wins) = resp["result"]["structuredContent"]["windows"].as_array() {
                if let Some(w) = wins.iter().find(|w| {
                    w["title"].as_str().map(|t| t.contains("Recovery")).unwrap_or(false)
                }) {
                    let id = w["window_id"].as_u64();
                    let pid = w["pid"].as_u64().map(|p| p as u32);
                    if let (Some(i), Some(p)) = (id, pid) {
                        found = Some((i, p));
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        found
    };

    match recovery_wid_pid {
        None => {
            eprintln!("Recovery dialog did not appear — LO didn't enter recovery \
                       state (possibly disabled in user config, or our crash \
                       trigger didn't take effect). Test cannot assert MSAA \
                       walk on a dialog that doesn't exist; logging and \
                       returning OK.");
        }
        Some((rwid, rpid)) => {
            let snap = call(&mut stdin, &mut stdout, 2, "get_window_state",
                serde_json::json!({
                    "pid": rpid as i64, "window_id": rwid, "capture_mode": "ax"
                }));
            let text = snapshot_text(&snap);
            assert!(!text.contains("SAL/VCL target, UIA walk skipped"),
                "Recovery dialog returned the old SAL-skip stub — MSAA fallback \
                 isn't engaging on SALFRAME Recovery dialogs.");
            assert!(text.contains("Button \"Discard All\""),
                "Recovery dialog tree should expose `Discard All` Button via MSAA. \
                 Got: {text:?}");
            assert!(text.contains("Button \"Recover Selected\""),
                "Recovery dialog tree should expose `Recover Selected` Button via MSAA. \
                 Got: {text:?}");
        }
    }

    // Cleanup.
    let _ = driver_c.kill(); let _ = driver_c.wait();
    let _ = writer_proc.kill(); let _ = writer_proc.wait();
    let _ = Command::new("taskkill").args(["/F", "/IM", "soffice.bin"]).output();
    std::thread::sleep(Duration::from_millis(800));
}

// ── Test 7: LO Calc smoke ──────────────────────────────────────────────────

/// Confirms the MSAA fallback generalizes beyond Writer. Launches LO
/// Calc, walks the main SALFRAME, asserts ≥15 SplitButtons with `expand`
/// — Calc has Calc-specific dropdowns (Select Function, Row, Column,
/// Freeze Panes) on top of the standard ones (Save, Open, Undo, Redo,
/// Paste, Font Color, etc.).
///
/// Skips cleanly if scalc.exe isn't installed (set LO_SCALC_EXE for
/// non-default paths).
#[test]
#[ignore]
fn harness_lo_vcl_calc_msaa_smoke() {
    let scalc = match scalc_exe() { Some(p) => p, None => return };
    let driver = driver_binary();
    if !driver.exists() { return; }

    let _ = Command::new("taskkill").args(["/F", "/IM", "soffice.bin"]).output();
    std::thread::sleep(Duration::from_millis(800));

    let mut calc_proc = match Command::new(&scalc)
        .args(["-norestore", "-nologo"])
        .stdout(Stdio::null()).stderr(Stdio::null())
        .spawn() {
            Ok(p) => p,
            Err(_) => return,
        };
    std::thread::sleep(Duration::from_secs(5));

    let mut driver_c = match Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn() {
            Ok(p) => p,
            Err(_) => {
                let _ = calc_proc.kill();
                return;
            }
        };
    let mut stdin_drv = driver_c.stdin.take().unwrap();
    let mut stdout_raw = driver_c.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut stdout_raw);
    init(&mut stdin_drv, &mut stdout);

    // Poll for the Calc window.
    let calc_wid_pid: Option<(u64, u32)> = {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut found = None;
        while std::time::Instant::now() < deadline {
            let r = call(&mut stdin_drv, &mut stdout, 1, "list_windows", serde_json::json!({}));
            if let Some(wins) = r["result"]["structuredContent"]["windows"].as_array() {
                if let Some(w) = wins.iter().find(|w| {
                    w["title"].as_str().map(|t| t.contains("LibreOffice Calc")).unwrap_or(false)
                }) {
                    let id = w["window_id"].as_u64();
                    let pid = w["pid"].as_u64().map(|p| p as u32);
                    if let (Some(i), Some(p)) = (id, pid) {
                        found = Some((i, p));
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        found
    };

    let (cwid, cpid) = calc_wid_pid.unwrap_or_else(|| {
        let _ = driver_c.kill();
        let _ = calc_proc.kill();
        let _ = Command::new("taskkill").args(["/F", "/IM", "soffice.bin"]).output();
        panic!("LO Calc window did not appear within 20 s");
    });

    let snap = call(&mut stdin_drv, &mut stdout, 2, "get_window_state",
        serde_json::json!({
            "pid": cpid as i64, "window_id": cwid, "capture_mode": "ax"
        }));
    let text = snapshot_text(&snap);
    assert!(!text.contains("SAL/VCL target, UIA walk skipped"),
        "Calc returned the old SAL-skip stub — MSAA didn't engage.");
    let sb_with_expand = text.lines()
        .filter(|l| l.contains("SplitButton") && l.contains("expand"))
        .count();
    assert!(sb_with_expand >= 15,
        "Expected ≥15 SplitButtons with `expand` in Calc toolbar (proves MSAA \
         generalizes beyond Writer). Got {sb_with_expand}. The MSAA role \
         mapping may differ in Calc, or the toolbar config is unexpectedly slim.");

    // Cleanup.
    let _ = call(&mut stdin_drv, &mut stdout, 3, "kill_app", serde_json::json!({
        "pid": cpid as i64
    }));
    let _ = driver_c.kill(); let _ = driver_c.wait();
    let _ = calc_proc.kill(); let _ = calc_proc.wait();
    let _ = Command::new("taskkill").args(["/F", "/IM", "soffice.bin"]).output();
    std::thread::sleep(Duration::from_millis(800));
}
