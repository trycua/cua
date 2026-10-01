//! Optional Windows check against a live LibreOffice Writer instance.
//!
//! This is the only coverage of the MSAA fallback that `platform-windows`
//! applies to SAL/VCL windows (`uia/mod.rs` SAL class detection and
//! `msaa.rs`). The repo-local WPF, WinUI3, and WebView2 harnesses all walk
//! through UIA, so none of them reaches that path. The row proves that a VCL
//! toolbar SplitButton exposes `expand` and that `click(action:"expand")`
//! opens its dropdown as a new top-level window.
//!
//! The canonical Windows image has no LibreOffice, so no runner selects this
//! file; `libs/cua-driver/tests/manual-e2e-allowlist.txt` records it as manual.
//! It fails, rather than passes, when Writer, the driver, or the Writer window
//! is missing.
//!
//! Run on an image with LibreOffice installed (override the path with
//! `LO_SWRITER_EXE`):
//!   cargo test -p cua-driver-e2e --test harness_libreoffice_test -- --ignored --nocapture

#![cfg(target_os = "windows")]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::{Driver, McpDriver};

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

fn setup() -> LoSession {
    let writer = swriter_exe()
        .expect("LibreOffice swriter.exe not found; install LibreOffice or set LO_SWRITER_EXE");

    // Sweep any leftover soffice.bin from a prior run so this test gets
    // a fresh writer pid + clean recovery state.
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "soffice.bin"])
        .output();
    std::thread::sleep(Duration::from_millis(800));

    let mut driver = McpDriver::spawn().expect("start the source-built driver");

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
        .expect("launch LibreOffice Writer");
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
                        return LoSession {
                            driver,
                            writer_pid: pid,
                            writer_wid: wid,
                        };
                    }
                }
            }
        }
        assert!(
            std::time::Instant::now() <= deadline,
            "Writer window did not appear within 30 s"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

// ── action:"expand" on a VCL SplitButton opens its dropdown ─────────────────

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
    let mut fx = setup();
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
            "capture_mode": "ax", "query": "Font Color"
        }),
    );
    let text = snap.text();
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

    let resp = driver.call(
        "click",
        serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "element_token": snap.element_token(idx),
            "action": "expand"
        }),
    );
    let resp_text = resp.text();
    assert!(
        resp_text.starts_with("✅"),
        "click(action:expand) failed: {resp_text:?}"
    );
    assert!(
        resp_text.contains("dropdown half"),
        "Expected response to mention dropdown half — got {resp_text:?}. \
         The MSAA dispatch path may not have triggered."
    );

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
