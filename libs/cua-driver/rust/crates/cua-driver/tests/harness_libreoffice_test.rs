//! Optional integration tests against a live LibreOffice Writer instance.
//!
//! This file is intentionally not part of the canonical Windows run-all path:
//! it requires LibreOffice to be installed on the machine. Run it explicitly on
//! images that include LibreOffice.
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
//! 2. **`harness_lo_vcl_font_color_expand_opens_picker`** — MSAA token
//!    targeting refuses without opening a window; a right-edge pixel click
//!    opens a SALTMPSUBFRAME picker.
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
//!   cargo test -p cua-driver --test harness_libreoffice_test -- --ignored --nocapture
//!
//! Tests skip cleanly if `swriter.exe` isn't on disk at one of the
//! standard install locations (override via `LO_SWRITER_EXE`).

#![cfg(target_os = "windows")]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::{Driver, McpDriver, ToolResponse};

fn msaa_pixel_click_args(
    pid: u32,
    wid: u64,
    state: &serde_json::Value,
    index: u64,
    dropdown: bool,
) -> serde_json::Value {
    let element = state["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|element| element["element_index"].as_u64() == Some(index))
        .expect("observed MSAA element must exist");
    let frame = &element["frame"];
    let bounds = &state["window_bounds"];
    let number = |value: &serde_json::Value| value.as_f64().expect("observed geometry must exist");
    let width = number(&frame["width"]);
    let height = number(&frame["height"]);
    assert!(width > 0.0 && height > 0.0);
    let x = number(&frame["x"])
        + if dropdown {
            width - 4.0_f64.min(width / 2.0)
        } else {
            width / 2.0
        };
    let y = number(&frame["y"]) + height / 2.0;
    let window_width = number(&bounds["width"]);
    let window_height = number(&bounds["height"]);
    let screenshot_width = number(&state["screenshot_width"]);
    let screenshot_height = number(&state["screenshot_height"]);
    assert!(
        window_width > 0.0
            && window_height > 0.0
            && screenshot_width > 0.0
            && screenshot_height > 0.0
    );
    serde_json::json!({
        "pid": pid, "window_id": wid, "delivery_mode": "foreground",
        "x": (x - number(&bounds["x"])) * screenshot_width / window_width,
        "y": (y - number(&bounds["y"])) * screenshot_height / window_height
    })
}

fn refuse_token_then_click_pixel(
    driver: &mut McpDriver,
    pid: u32,
    wid: u64,
    snapshot: &ToolResponse,
    index: u64,
    dropdown: bool,
) -> ToolResponse {
    let window_ids = |driver: &mut McpDriver| {
        let windows = driver.call("list_windows", serde_json::json!({"pid": pid}));
        assert!(!windows.is_error(), "{}", windows.text());
        windows.structured()["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|window| window["window_id"].as_u64().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
    };
    let before = window_ids(driver);
    let mut args = serde_json::json!({"pid": pid, "window_id": wid});
    args.as_object_mut()
        .unwrap()
        .extend(snapshot.element_target(index));
    let refused = driver.call("click", args);
    assert!(
        refused.is_error(),
        "MSAA token unexpectedly accepted: {}",
        refused.text()
    );
    assert_eq!(
        refused.structured()["refusal"]["code"],
        "element_resolution_failed"
    );
    assert!(refused.structured()["refusal"]["message"]
        .as_str()
        .unwrap()
        .contains("incomplete accessibility tree"));
    assert_eq!(
        window_ids(driver),
        before,
        "refused token changed the open windows"
    );
    driver.call(
        "click",
        msaa_pixel_click_args(pid, wid, snapshot.structured(), index, dropdown),
    )
}

#[test]
fn msaa_pixel_targets_use_observed_frame_and_screenshot_scale() {
    let snapshot = serde_json::json!({
        "window_bounds": {"x": -200, "y": 100, "width": 400, "height": 200},
        "screenshot_width": 200, "screenshot_height": 100,
        "elements": [{"element_index": 9, "frame": {"x": -160, "y": 120, "width": 40, "height": 20}}]
    });
    assert_eq!(
        msaa_pixel_click_args(7, 8, &snapshot, 9, true),
        serde_json::json!({
            "pid": 7, "window_id": 8, "delivery_mode": "foreground", "x": 38.0, "y": 15.0
        })
    );
    assert_eq!(msaa_pixel_click_args(7, 8, &snapshot, 9, false)["x"], 30.0);
}

#[test]
#[should_panic(expected = "observed MSAA element must exist")]
fn msaa_pixel_target_never_substitutes_a_different_row() {
    let snapshot = serde_json::json!({"elements": [{"element_index": 9}]});
    msaa_pixel_click_args(7, 8, &snapshot, 0, false);
}

// ── LO paths ─────────────────────────────────────────────────────────────────

/// LO Writer executable. Honour `LO_SWRITER_EXE` env override (for CI
/// images with non-default install paths), otherwise probe the two
/// standard locations.
fn swriter_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LO_SWRITER_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    for candidate in [
        r"C:\Program Files\LibreOffice\program\swriter.exe",
        r"C:\Program Files (x86)\LibreOffice\program\swriter.exe",
    ] {
        let pb = PathBuf::from(candidate);
        if pb.exists() {
            return Some(pb);
        }
    }
    None
}

/// LO Calc executable, same probe shape as `swriter_exe()`.
fn scalc_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LO_SCALC_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    for candidate in [
        r"C:\Program Files\LibreOffice\program\scalc.exe",
        r"C:\Program Files (x86)\LibreOffice\program\scalc.exe",
    ] {
        let pb = PathBuf::from(candidate);
        if pb.exists() {
            return Some(pb);
        }
    }
    None
}

// ── fixture ──────────────────────────────────────────────────────────────────

/// Live LO Writer session: the long-lived MCP driver plus the located
/// Writer window. The driver's reaper kills the driver and the spawned
/// writer on drop; the extra soffice.bin sweep below clears the SAL/VCL
/// background daemon that outlives the launcher so the next test starts
/// clean.
struct LoSession {
    driver: McpDriver,
    writer_pid: u32,
    writer_wid: u64,
}

impl Drop for LoSession {
    fn drop(&mut self) {
        // SAL/VCL soffice.bin daemon hangs around after the parent exits;
        // best-effort sweep via taskkill so the next test starts clean.
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "soffice.bin"])
            .output();
        std::thread::sleep(Duration::from_millis(800));
    }
}

fn setup() -> Option<LoSession> {
    let writer = match swriter_exe() {
        Some(p) => p,
        None => {
            eprintln!("LibreOffice swriter.exe not found — set LO_SWRITER_EXE or install LO");
            return None;
        }
    };

    // Sweep any leftover soffice.bin from a prior run so this test gets
    // a fresh writer pid + clean recovery state.
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "soffice.bin"])
        .output();
    std::thread::sleep(Duration::from_millis(800));

    let mut driver = McpDriver::spawn()?;

    // `-norestore` skips the Document Recovery dialog (which would
    // block the Writer top-level window from ever appearing if the
    // previous LO session crashed). `-nologo` skips the splash screen.
    driver
        .reaper()
        .spawn(
            Command::new(&writer)
                .args(["-norestore", "-nologo"])
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .ok()?;
    // LO launches via soffice.bin background daemon, then forks the
    // actual writer process. The pid we just spawned is the launcher;
    // the real writer is a child. Poll list_apps via cua-driver to
    // find it.
    std::thread::sleep(Duration::from_secs(3));

    // Find the LO Writer window by title — its owning pid is the
    // soffice.bin daemon (NOT the launcher pid above). Poll up to
    // 30 s for the recovery dialog (if any) + main window to appear.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let resp = driver.call("list_windows", serde_json::json!({}));
        if let Some(wins) = resp.structured()["windows"].as_array() {
            for w in wins {
                let title = w["title"].as_str().unwrap_or("");
                let app = w["app_name"].as_str().unwrap_or("");
                if app.contains("soffice") && title.contains("LibreOffice Writer") {
                    let pid = w["pid"].as_u64().unwrap_or(0) as u32;
                    let wid = w["window_id"].as_u64().unwrap_or(0);
                    if pid > 0 && wid > 0 {
                        return Some(LoSession {
                            driver,
                            writer_pid: pid,
                            writer_wid: wid,
                        });
                    }
                }
            }
        }
        if std::time::Instant::now() > deadline {
            eprintln!("Writer window did not appear within 30 s");
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
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
/// `actions=[invoke,expand]`. This is observation metadata, not proof that
/// token targeting is supported: MSAA cannot currently attest completeness.
/// The action tests below verify refusal, then explicitly click observed pixels.
///
/// If this fails: either the MSAA path stopped applying to SALFRAME
/// (check `uia/mod.rs` SAL class detection), or LO changed its
/// toolbar role (no longer ROLE_SYSTEM_BUTTONDROPDOWN).
#[test]
#[ignore]
fn harness_lo_vcl_font_color_split_button_exposes_expand() {
    let mut fx = match setup() {
        Some(s) => s,
        None => return,
    };
    let (pid, wid) = (fx.writer_pid, fx.writer_wid);
    let driver = &mut fx.driver;

    let snap = driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "capture_mode": "ax", "query": "Font Color"
        }),
    );
    let text = snap.text();
    assert!(
        text.contains("SplitButton \"Font Color\""),
        "Font Color SplitButton not found in tree: {text:?}"
    );
    let line = text
        .lines()
        .find(|l| l.contains("\"Font Color\""))
        .unwrap_or("");
    assert!(
        line.contains("expand"),
        "Expected `expand` in actions on Font Color SplitButton — got {line:?}. \
         The MSAA fallback may not be running for SALFRAME (check uia/mod.rs \
         SAL class detection — should route ALL SAL* classes through msaa.rs)."
    );
    assert!(
        line.contains("invoke"),
        "Expected `invoke` to remain in actions alongside `expand` — got {line:?}. \
         MSAA walker should expose both press and dropdown halves."
    );
}

// ── Test 2: explicit pixel fallback opens the color picker ─────────────────

/// MSAA token targeting must refuse without opening the picker. A subsequent
/// explicit foreground pixel click on the observed dropdown arrow must open
/// the same SALTMPSUBFRAME picker as before; refusal alone is not a passing test.
#[test]
#[ignore]
fn harness_lo_vcl_font_color_expand_opens_picker() {
    let mut fx = match setup() {
        Some(s) => s,
        None => return,
    };
    let (pid, wid) = (fx.writer_pid, fx.writer_wid);
    let driver = &mut fx.driver;

    // Bring Writer to foreground so SendInput click lands on it.
    let _ = driver.call(
        "bring_to_front",
        serde_json::json!({
            "pid": pid as i64, "window_id": wid
        }),
    );
    std::thread::sleep(Duration::from_millis(400));

    let snap = driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "include_screenshot": true, "query": "Font Color"
        }),
    );
    let text = snap.tree_text();
    let line = text
        .lines()
        .find(|l| l.contains("\"Font Color\"") && l.contains("expand"))
        .unwrap_or_else(|| {
            panic!("Font Color SplitButton with `expand` action not found: {text:?}")
        });
    let s = line.find('[').expect("element_index bracket open");
    let e = line[s..].find(']').expect("element_index bracket close") + s;
    let idx: u64 = line[s + 1..e]
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("could not parse element_index from line {line:?}"));

    // Snapshot windows under our pid BEFORE the click.
    let before = driver.call(
        "list_windows",
        serde_json::json!({
            "pid": pid as i64
        }),
    );
    let before_ids: std::collections::HashSet<u64> = before.structured()["windows"]
        .as_array()
        .map(|a| a.iter().filter_map(|w| w["window_id"].as_u64()).collect())
        .unwrap_or_default();

    let resp = refuse_token_then_click_pixel(driver, pid, wid, &snap, idx, true);
    assert!(
        !resp.is_error(),
        "dropdown pixel click failed: {}",
        resp.text()
    );
    assert_eq!(resp.action_delivery_mode(), Some("foreground"));

    // Let the picker spawn.
    std::thread::sleep(Duration::from_millis(900));

    let after = driver.call(
        "list_windows",
        serde_json::json!({
            "pid": pid as i64
        }),
    );
    let new_wins: Vec<&serde_json::Value> = after.structured()["windows"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|w| {
                    let id = w["window_id"].as_u64();
                    id.map(|i| !before_ids.contains(&i)).unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    let picker = new_wins.iter().find(|w| {
        w["title"]
            .as_str()
            .map(|t| t.contains("Font Color"))
            .unwrap_or(false)
    });
    assert!(
        picker.is_some(),
        "No new window titled 'Font Color' appeared after click(action:expand). \
         new_windows={new_wins:?}. The right-edge dispatch may have hit the \
         wrong pixel (LO's toolbar scaled differently?) or the picker spawned \
         as a child window instead of a top-level."
    );
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
    let mut fx = match setup() {
        Some(s) => s,
        None => return,
    };
    let (pid, wid) = (fx.writer_pid, fx.writer_wid);
    let driver = &mut fx.driver;

    std::thread::sleep(Duration::from_millis(800));

    let open_resp = driver.call(
        "hotkey",
        serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "keys": ["ctrl", "h"], "delivery_mode": "foreground"
        }),
    );
    let open_text = open_resp.text();
    assert!(
        open_text.starts_with("✅"),
        "hotkey(ctrl+h, foreground) failed: {open_text:?}"
    );

    let dialog_wid = {
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        loop {
            let wins = driver.call(
                "list_windows",
                serde_json::json!({
                    "pid": pid as i64
                }),
            );
            let found = wins.structured()["windows"].as_array().and_then(|a| {
                a.iter().find_map(|w| {
                    let title = w["title"].as_str().unwrap_or("");
                    let id = w["window_id"].as_u64();
                    if id != Some(wid) && (title.contains("Find") || title.contains("Replace")) {
                        id
                    } else {
                        None
                    }
                })
            });
            if let Some(id) = found {
                break id;
            }
            if std::time::Instant::now() > deadline {
                panic!(
                    "Find & Replace dialog did not appear after Ctrl+H — \
                        Writer accelerator may have regressed under \
                        foreground SendInput on SALFRAME."
                );
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    };

    // Snapshot the dialog — MSAA walker should produce a real tree
    // with the dialog's buttons and edit fields, NOT the old skip
    // stub.
    let dialog_ax = driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": pid as i64, "window_id": dialog_wid, "capture_mode": "ax"
        }),
    );
    let dialog_text = dialog_ax.text();
    assert!(
        !dialog_text.contains("SAL/VCL target, UIA walk skipped"),
        "Got the old skip-stub message — MSAA fallback did not engage on \
         this SALSUBFRAME. Snapshot was: {dialog_text:?}"
    );
    assert!(
        dialog_text.contains("Button \"Close\""),
        "Expected MSAA walker to expose a 'Close' Button in the Find & Replace \
         dialog tree (it's a documented child via accChild). Snapshot was: {dialog_text:?}"
    );
    // Sanity: should have several actionable element_indices, not zero.
    let actionable_count = dialog_text
        .lines()
        .filter(|l| l.contains("actions=[invoke"))
        .count();
    assert!(
        actionable_count >= 4,
        "Expected ≥4 actionable elements in Find & Replace via MSAA walker, \
         got {actionable_count}. Tree: {dialog_text:?}"
    );

    // Foreground Escape should close the dialog.
    let esc_resp = driver.call(
        "press_key",
        serde_json::json!({
            "pid": pid as i64, "window_id": dialog_wid,
            "key": "escape", "delivery_mode": "foreground"
        }),
    );
    let esc_text = esc_resp.text();
    assert!(
        esc_text.starts_with("✅"),
        "press_key(escape, foreground) returned an error: {esc_text:?}"
    );
    std::thread::sleep(Duration::from_millis(700));

    let wins_after = driver.call(
        "list_windows",
        serde_json::json!({
            "pid": pid as i64
        }),
    );
    let still_open = wins_after.structured()["windows"]
        .as_array()
        .map(|a| {
            a.iter()
                .any(|w| w["window_id"].as_u64() == Some(dialog_wid))
        })
        .unwrap_or(false);
    assert!(
        !still_open,
        "Find & Replace dialog stayed open after foreground Escape — \
         SAL modal input dispatch regressed. Check uia/mod.rs SAL-class \
         handling and the press_key SendInput path."
    );

    // Cleanup: kill the LO process — Drop impl sweeps soffice.bin.
    let _ = driver.call(
        "kill_app",
        serde_json::json!({
            "pid": pid as i64
        }),
    );
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
    let mut fx = match setup() {
        Some(s) => s,
        None => return,
    };
    let (pid, wid) = (fx.writer_pid, fx.writer_wid);
    let driver = &mut fx.driver;

    let snap = driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": pid as i64, "window_id": wid, "capture_mode": "ax"
        }),
    );
    let text = snap.text();
    let splitbutton_lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("SplitButton") && l.contains("actions=["))
        .collect();
    let with_expand: Vec<&&str> = splitbutton_lines
        .iter()
        .filter(|l| l.contains("expand"))
        .collect();
    assert!(
        splitbutton_lines.len() >= 15,
        "Expected ≥15 SplitButtons in LO Writer toolbar tree, got {}. \
         Possible MSAA walker regression. Full snapshot first 1500 chars: {}",
        splitbutton_lines.len(),
        &text.chars().take(1500).collect::<String>()
    );
    assert_eq!(
        with_expand.len(),
        splitbutton_lines.len(),
        "{}/{} SplitButtons report `expand`. The MSAA role→actions \
         mapping may have dropped one of BUTTONDROPDOWN / BUTTONMENU / \
         BUTTONDROPDOWNGRID / SPLITBUTTON. Missing-expand lines:\n{}",
        with_expand.len(),
        splitbutton_lines.len(),
        splitbutton_lines
            .iter()
            .filter(|l| !l.contains("expand"))
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join("\n")
    );
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
    let mut fx = match setup() {
        Some(s) => s,
        None => return,
    };
    let (pid, wid) = (fx.writer_pid, fx.writer_wid);
    let driver = &mut fx.driver;

    // Foreground first so type_text + Ctrl+A land on Writer.
    let _ = driver.call(
        "bring_to_front",
        serde_json::json!({
            "pid": pid as i64, "window_id": wid
        }),
    );
    std::thread::sleep(Duration::from_millis(400));

    let _ = driver.call(
        "type_text",
        serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "text": "color pick regression"
        }),
    );
    std::thread::sleep(Duration::from_millis(400));

    let _ = driver.call(
        "hotkey",
        serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "keys": ["ctrl", "a"], "delivery_mode": "foreground"
        }),
    );
    std::thread::sleep(Duration::from_millis(400));

    // Locate Font Color element_index in Writer's MSAA tree.
    let snap = driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "include_screenshot": true, "query": "Font Color"
        }),
    );
    let snap_text = snap.tree_text();
    let fc_line = snap_text
        .lines()
        .find(|l| l.contains("\"Font Color\"") && l.contains("expand"))
        .unwrap_or_else(|| panic!("Font Color with `expand` not found in tree: {snap_text:?}"));
    let s = fc_line.find('[').expect("[ in fc_line");
    let e = fc_line[s..].find(']').expect("] in fc_line") + s;
    let fc_idx: u64 = fc_line[s + 1..e].trim().parse().expect("fc element_index");

    // Snapshot which windows existed BEFORE opening the picker so
    // we can isolate the picker on appearance.
    let before = driver.call(
        "list_windows",
        serde_json::json!({
            "pid": pid as i64
        }),
    );
    let before_ids: std::collections::HashSet<u64> = before.structured()["windows"]
        .as_array()
        .map(|a| a.iter().filter_map(|w| w["window_id"].as_u64()).collect())
        .unwrap_or_default();

    // Token targeting refuses; explicitly open the observed dropdown by pixel.
    let open_resp = refuse_token_then_click_pixel(driver, pid, wid, &snap, fc_idx, true);
    assert!(
        !open_resp.is_error(),
        "dropdown pixel click failed: {}",
        open_resp.text()
    );
    std::thread::sleep(Duration::from_millis(900));

    // Find the picker window.
    let after = driver.call(
        "list_windows",
        serde_json::json!({
            "pid": pid as i64
        }),
    );
    let picker_wid = after.structured()["windows"]
        .as_array()
        .and_then(|a| {
            a.iter().find_map(|w| {
                let id = w["window_id"].as_u64();
                let title = w["title"].as_str().unwrap_or("");
                if id.map(|i| !before_ids.contains(&i)).unwrap_or(false)
                    && title.contains("Font Color")
                {
                    id
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| panic!("No new 'Font Color' picker window appeared"));

    // Walk the picker tree to find a green color cell.
    let psnap = driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": pid as i64, "window_id": picker_wid, "include_screenshot": true
        }),
    );
    let psnap_text = psnap.tree_text();
    let green_line = psnap_text
        .lines()
        .find(|l| l.contains("\"Green\"") && l.contains("[") && l.contains("actions=[invoke"))
        .unwrap_or_else(|| {
            panic!(
                "No 'Green' ListItem in picker tree. The MSAA walker may have \
             truncated the color grid or LO's locale renamed the color. \
             First 1500 chars: {}",
                &psnap_text.chars().take(1500).collect::<String>()
            )
        });
    let s = green_line.find('[').expect("[ in green");
    let e = green_line[s..].find(']').expect("] in green") + s;
    let green_idx: u64 = green_line[s + 1..e]
        .trim()
        .parse()
        .expect("green element_index");

    // Refusal must leave the picker open; the explicit center pixel picks Green.
    let pick_resp =
        refuse_token_then_click_pixel(driver, pid, picker_wid, &psnap, green_idx, false);
    assert!(
        !pick_resp.is_error(),
        "Green pixel click failed: {}",
        pick_resp.text()
    );
    std::thread::sleep(Duration::from_millis(700));

    // Verify the picker closed (LO closes the popup after a color is
    // selected — the canonical "color applied" signal). If it
    // didn't, the click didn't actually land on the cell.
    let after2 = driver.call(
        "list_windows",
        serde_json::json!({
            "pid": pid as i64
        }),
    );
    let still_open = after2.structured()["windows"]
        .as_array()
        .map(|a| {
            a.iter()
                .any(|w| w["window_id"].as_u64() == Some(picker_wid))
        })
        .unwrap_or(false);
    assert!(
        !still_open,
        "Picker stayed open after click on Green — the cell click \
         didn't land. LO didn't apply the color."
    );
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
    let writer = match swriter_exe() {
        Some(p) => p,
        None => return,
    };
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "soffice.bin"])
        .output();
    std::thread::sleep(Duration::from_millis(800));

    // Launch (no -norestore, no -nologo flag → default behavior).
    let mut crash_proc = match Command::new(&writer)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(p) => p,
        Err(_) => return,
    };
    std::thread::sleep(Duration::from_secs(5));
    let _ = crash_proc.kill();
    let _ = crash_proc.wait();
    // Force-kill the actual soffice.bin worker too (the launcher is what
    // we just killed; the real LO process is a fork).
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "soffice.bin"])
        .output();
    std::thread::sleep(Duration::from_secs(2));

    // Re-launch Writer — Recovery dialog should appear.
    let Some(mut driver) = McpDriver::spawn() else {
        return;
    };
    if driver
        .reaper()
        .spawn(
            Command::new(&writer)
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .is_err()
    {
        return;
    }
    std::thread::sleep(Duration::from_secs(5));

    // Poll for the Recovery dialog.
    let recovery_wid_pid: Option<(u64, u32)> = {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut found = None;
        while std::time::Instant::now() < deadline {
            let resp = driver.call("list_windows", serde_json::json!({}));
            if let Some(wins) = resp.structured()["windows"].as_array() {
                if let Some(w) = wins.iter().find(|w| {
                    w["title"]
                        .as_str()
                        .map(|t| t.contains("Recovery"))
                        .unwrap_or(false)
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
            eprintln!(
                "Recovery dialog did not appear — LO didn't enter recovery \
                       state (possibly disabled in user config, or our crash \
                       trigger didn't take effect). Test cannot assert MSAA \
                       walk on a dialog that doesn't exist; logging and \
                       returning OK."
            );
        }
        Some((rwid, rpid)) => {
            let snap = driver.call(
                "get_window_state",
                serde_json::json!({
                    "pid": rpid as i64, "window_id": rwid, "capture_mode": "ax"
                }),
            );
            let text = snap.text();
            assert!(
                !text.contains("SAL/VCL target, UIA walk skipped"),
                "Recovery dialog returned the old SAL-skip stub — MSAA fallback \
                 isn't engaging on SALFRAME Recovery dialogs."
            );
            assert!(
                text.contains("Button \"Discard All\""),
                "Recovery dialog tree should expose `Discard All` Button via MSAA. \
                 Got: {text:?}"
            );
            assert!(
                text.contains("Button \"Recover Selected\""),
                "Recovery dialog tree should expose `Recover Selected` Button via MSAA. \
                 Got: {text:?}"
            );
        }
    }

    // Cleanup: reaper kills the MCP driver + the spawned writer on drop;
    // sweep the soffice.bin daemon that outlives the launcher.
    drop(driver);
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "soffice.bin"])
        .output();
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
    let scalc = match scalc_exe() {
        Some(p) => p,
        None => return,
    };

    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "soffice.bin"])
        .output();
    std::thread::sleep(Duration::from_millis(800));

    let Some(mut driver) = McpDriver::spawn() else {
        return;
    };
    if driver
        .reaper()
        .spawn(
            Command::new(&scalc)
                .args(["-norestore", "-nologo"])
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .is_err()
    {
        return;
    }
    std::thread::sleep(Duration::from_secs(5));

    // Poll for the Calc window.
    let calc_wid_pid: Option<(u64, u32)> = {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut found = None;
        while std::time::Instant::now() < deadline {
            let r = driver.call("list_windows", serde_json::json!({}));
            if let Some(wins) = r.structured()["windows"].as_array() {
                if let Some(w) = wins.iter().find(|w| {
                    w["title"]
                        .as_str()
                        .map(|t| t.contains("LibreOffice Calc"))
                        .unwrap_or(false)
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

    let (cwid, cpid) = match calc_wid_pid {
        Some(v) => v,
        None => {
            let _ = Command::new("taskkill")
                .args(["/F", "/IM", "soffice.bin"])
                .output();
            panic!("LO Calc window did not appear within 20 s");
        }
    };

    let snap = driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": cpid as i64, "window_id": cwid, "capture_mode": "ax"
        }),
    );
    let text = snap.text();
    assert!(
        !text.contains("SAL/VCL target, UIA walk skipped"),
        "Calc returned the old SAL-skip stub — MSAA didn't engage."
    );
    let sb_with_expand = text
        .lines()
        .filter(|l| l.contains("SplitButton") && l.contains("expand"))
        .count();
    assert!(
        sb_with_expand >= 15,
        "Expected ≥15 SplitButtons with `expand` in Calc toolbar (proves MSAA \
         generalizes beyond Writer). Got {sb_with_expand}. The MSAA role \
         mapping may differ in Calc, or the toolbar config is unexpectedly slim."
    );

    // Cleanup: reaper kills the MCP driver + the spawned calc on drop;
    // sweep the soffice.bin daemon that outlives the launcher.
    let _ = driver.call(
        "kill_app",
        serde_json::json!({
            "pid": cpid as i64
        }),
    );
    drop(driver);
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "soffice.bin"])
        .output();
    std::thread::sleep(Duration::from_millis(800));
}
