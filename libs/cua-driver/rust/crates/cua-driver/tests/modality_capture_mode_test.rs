//! modality_capture_mode_test — the **capture_mode** axis (`ax` / `vision` /
//! `som`) of the modality matrix, asserted on each platform's native
//! controlled-harness app.
//!
//! This generalizes the Windows-only oracle in `modality_background_test`
//! (`capture_mode_ax_returns_tree_only` / `capture_mode_vision_returns_image_only`)
//! to macOS (AppKit) and Linux (GTK3), closing the audited gap where the
//! `capture_mode` axis was only exercised on Windows. The matrix it covers is
//! documented in `docs/content/docs/explanation/capture-and-dispatch-modalities.mdx`.
//!
//! Oracle, against the harness window:
//!   * `capture_mode=ax`     → accessibility tree present (`btn-increment`),
//!                             NO `image` content entry.
//!   * `capture_mode=vision` → `image` content present, NO tree markdown.
//!   * `capture_mode=som`    → both (the default).
//!
//! Caveats handled as graceful skips (these are `#[ignore]` interactive tests,
//! consistent with the rest of the harness suite):
//!   * Native **vision** capture needs a screen-capture grant (Screen Recording
//!     on macOS). Without it the driver returns a tree but no PNG, so the image
//!     assertions skip-with-note rather than false-fail.
//!   * The **ax** tree needs the platform accessibility grant (TCC on macOS,
//!     AT-SPI bus on Linux). An empty tree skips the tree assertion.
//!
//! Run explicitly:
//!   cargo test -p cua-driver --test modality_capture_mode_test -- --ignored --nocapture --test-threads=1

#![cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::{harness_app, Driver, McpDriver, ToolResponse};

/// The aid every harness exposes on its increment button — the tree marker
/// (WPF AutomationId / AppKit AX identifier / GTK3 AT-SPI accessible name).
const TREE_MARKER: &str = "btn-increment";

/// Does the response carry a screenshot? Checks both the MCP `image` content
/// entry and the canonical `screenshot_png_b64` structured field (the CLI path
/// surfaces only the latter), so the oracle is transport-agnostic.
fn has_image(resp: &ToolResponse) -> bool {
    let mcp_image = resp.raw["result"]["content"]
        .as_array()
        .map(|a| a.iter().any(|c| c["type"].as_str() == Some("image")))
        .unwrap_or(false);
    let structured_png = resp.structured()["screenshot_png_b64"]
        .as_str()
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    mcp_image || structured_png
}

/// Is the accessibility tree present (the increment-button marker rendered)?
/// Checks the canonical `tree_markdown` structured field — `som` puts a short
/// summary in the MCP text block but the full tree only in `tree_markdown`, so
/// asserting on `text()` would under-report the tree in `som` mode.
fn tree_has_marker(resp: &ToolResponse) -> bool {
    resp.structured()["tree_markdown"]
        .as_str()
        .map(|t| t.contains(TREE_MARKER))
        .unwrap_or(false)
        || resp.text().contains(TREE_MARKER)
}

fn snapshot(driver: &mut McpDriver, pid: u32, wid: u64, mode: &str) -> ToolResponse {
    driver.call(
        "get_window_state",
        serde_json::json!({ "pid": pid as i64, "window_id": wid, "capture_mode": mode }),
    )
}

/// Snapshot, retrying until the accessibility tree has populated (the marker
/// appears) or the attempts are exhausted. The AX/AT-SPI tree can lag a
/// cold-launched window by several seconds: macOS AX registration is a few
/// hundred ms, but a freshly launched GTK3 harness needs the AT-SPI atk-bridge
/// to register on the a11y bus, which on a cold bus (CI cold boot) can take a
/// few seconds. The budget is ~8s so the tree-bearing modes (`ax`, `som`)
/// reliably exercise their assertions instead of degrading to a cold-start skip.
fn snapshot_settled(driver: &mut McpDriver, pid: u32, wid: u64, mode: &str) -> ToolResponse {
    let mut resp = snapshot(driver, pid, wid, mode);
    for _ in 0..16 {
        if tree_has_marker(&resp) {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
        resp = snapshot(driver, pid, wid, mode);
    }
    resp
}

// ── per-platform harness launch → (pid, window_id) ──────────────────────────────
//
// Each `launch` records the harness window pids that already exist, spawns its
// own instance, then resolves the window whose pid is NEW. This keeps the three
// tests deterministic when run back-to-back (`--test-threads=1`): a prior test's
// window can linger past its driver's teardown, and resolving by title alone
// would race onto that stale instance.

#[cfg(target_os = "windows")]
fn launch(driver: &mut McpDriver) -> Option<(u32, u64)> {
    let exe = harness_app("harness-wpf", "CuaTestHarness.Wpf.exe");
    launch_new_instance(driver, &exe, "CuaTestHarness")
}

#[cfg(target_os = "linux")]
fn launch(driver: &mut McpDriver) -> Option<(u32, u64)> {
    let exe = std::env::var("HARNESS_GTK3_EXE")
        .map(std::path::PathBuf::from)
        .ok()
        .filter(|p| p.exists())
        .unwrap_or_else(|| harness_app("harness-gtk3", "CuaTestHarness.Gtk3"));
    launch_new_instance(driver, &exe, "CuaTestHarness GTK3")
}

#[cfg(target_os = "macos")]
fn launch(driver: &mut McpDriver) -> Option<(u32, u64)> {
    let app = std::env::var("HARNESS_APPKIT_APP")
        .map(std::path::PathBuf::from)
        .ok()
        .filter(|p| p.exists())
        .unwrap_or_else(|| harness_app("harness-appkit", "CuaTestHarness.AppKit.app"));
    // Launch the binary inside the `.app` directly so the reaper owns the pid.
    let exe = app.join("Contents/MacOS/CuaTestHarness.AppKit");
    launch_new_instance(driver, &exe, "CuaTestHarness AppKit")
}

/// Spawn `exe` under the driver's reaper and resolve the window of the *new*
/// instance (its pid was not among the harness windows before the spawn).
fn launch_new_instance(driver: &mut McpDriver, exe: &std::path::Path, title: &str) -> Option<(u32, u64)> {
    if !exe.exists() {
        eprintln!("[capture_mode] harness not built ({exe:?}) — skipping");
        return None;
    }
    let before = harness_pids(driver, title);
    driver
        .reaper()
        .spawn(Command::new(exe).stdout(Stdio::null()).stderr(Stdio::null()))
        .ok()?;
    resolve_new_window(driver, title, &before)
}

/// The pids of all currently-open windows whose title contains `title`.
fn harness_pids(driver: &mut McpDriver, title: &str) -> std::collections::HashSet<u32> {
    let mut pids = std::collections::HashSet::new();
    let r = driver.call("list_windows", serde_json::json!({}));
    if let Some(wins) = r.structured()["windows"].as_array() {
        for w in wins {
            if w["title"].as_str().unwrap_or("").contains(title) {
                if let Some(pid) = w["pid"].as_u64() {
                    pids.insert(pid as u32);
                }
            }
        }
    }
    pids
}

/// Poll `list_windows` until a `title` window whose pid is NOT in `before`
/// appears; returns (pid, window_id) and tracks the pid with the reaper.
fn resolve_new_window(
    driver: &mut McpDriver,
    title: &str,
    before: &std::collections::HashSet<u32>,
) -> Option<(u32, u64)> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let r = driver.call("list_windows", serde_json::json!({}));
        if let Some(wins) = r.structured()["windows"].as_array() {
            for w in wins {
                if !w["title"].as_str().unwrap_or("").contains(title) {
                    continue;
                }
                let pid = w["pid"].as_u64().unwrap_or(0) as u32;
                let wid = w["window_id"].as_u64().unwrap_or(0);
                if pid != 0 && wid != 0 && !before.contains(&pid) {
                    driver.reaper().track_pid(pid);
                    return Some((pid, wid));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    eprintln!("[capture_mode] new harness window {title:?} never appeared — is a graphical session available? skipping");
    None
}

// ── tests ───────────────────────────────────────────────────────────────────────

/// `capture_mode=ax`: tree present, NO image content.
#[test]
#[ignore]
fn capture_mode_ax_returns_tree_only() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some((pid, wid)) = launch(&mut driver) else { return };

    let resp = snapshot_settled(&mut driver, pid, wid, "ax");
    assert!(!resp.is_error(), "get_window_state(ax) errored: {}", resp.text());

    if !tree_has_marker(&resp) {
        eprintln!("[capture_mode] ax tree has no {TREE_MARKER:?} — accessibility grant likely missing; skipping");
        return;
    }
    assert!(
        !has_image(&resp),
        "capture_mode=ax must NOT return an image content entry: {}",
        resp.text().chars().take(160).collect::<String>()
    );
    println!("✅ capture_mode=ax: tree present ({TREE_MARKER}), no image");
}

/// `capture_mode=vision`: image content present, NO tree markdown.
#[test]
#[ignore]
fn capture_mode_vision_returns_image_only() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some((pid, wid)) = launch(&mut driver) else { return };

    let resp = snapshot(&mut driver, pid, wid, "vision");
    assert!(!resp.is_error(), "get_window_state(vision) errored: {}", resp.text());

    if !has_image(&resp) {
        eprintln!("[capture_mode] vision returned no image — screen-capture grant likely missing; skipping");
        return;
    }
    assert!(
        !tree_has_marker(&resp),
        "capture_mode=vision must NOT return tree markdown (found {TREE_MARKER}): {}",
        resp.text().chars().take(160).collect::<String>()
    );
    println!("✅ capture_mode=vision: image present, no tree");
}

/// `capture_mode=som` (default): both tree and image present (each asserted only
/// when its grant is available, so the test is a true superset of ax + vision).
#[test]
#[ignore]
fn capture_mode_som_returns_tree_and_image() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some((pid, wid)) = launch(&mut driver) else { return };

    let resp = snapshot_settled(&mut driver, pid, wid, "som");
    assert!(!resp.is_error(), "get_window_state(som) errored: {}", resp.text());

    let img = has_image(&resp);
    let tree = tree_has_marker(&resp);
    assert!(
        img || tree,
        "capture_mode=som returned neither tree nor image — both grants missing? {}",
        resp.text().chars().take(160).collect::<String>()
    );
    if !img {
        eprintln!("[capture_mode] som: no image (screen-capture grant missing) — tree-only check");
    }
    if !tree {
        eprintln!("[capture_mode] som: no tree marker (accessibility grant missing) — image-only check");
    }
    println!("✅ capture_mode=som: tree={tree} image={img}");
}
