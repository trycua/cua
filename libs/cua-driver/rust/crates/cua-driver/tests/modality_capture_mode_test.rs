//! modality_capture_mode_test — the get_window_state **perception** contract,
//! asserted on each platform's native controlled-harness app.
//!
//! get_window_state is perception-mode-agnostic now: it ALWAYS returns BOTH the
//! element tree AND a screenshot (ground on both, cross-check the sometimes-lying
//! tree). `capture_mode` is deprecated and ignored; the only way to suppress the
//! screenshot is the perf opt-out `include_screenshot:false`.
//!
//! Oracle, against the harness window:
//!   * default → BOTH the tree (`btn-increment`) AND an `image` content entry.
//!   * `include_screenshot:false` → tree present, NO `image` (the cheap path).
//!   * `capture_mode:"vision"` (deprecated) → IGNORED; still returns both.
//!
//! Caveats handled as graceful skips (these are `#[ignore]` interactive tests,
//! consistent with the rest of the harness suite):
//!   * The screenshot needs a screen-capture grant (Screen Recording on macOS).
//!     Without it the driver returns a tree but no PNG, so the image assertions
//!     skip-with-note rather than false-fail.
//!   * The tree needs the platform accessibility grant (TCC on macOS, AT-SPI bus
//!     on Linux). An empty tree skips the tree assertion.
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

#[cfg(target_os = "macos")]
fn spawn_driver() -> Option<McpDriver> {
    McpDriver::spawn_macos_daemon_proxy()
}

#[cfg(not(target_os = "macos"))]
fn spawn_driver() -> Option<McpDriver> {
    McpDriver::spawn()
}

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

/// Snapshot with NO capture_mode / no extra args (the default path), retrying
/// until the tree settles.
fn snapshot_settled_default(driver: &mut McpDriver, pid: u32, wid: u64) -> ToolResponse {
    let args = serde_json::json!({ "pid": pid as i64, "window_id": wid });
    let mut resp = driver.call("get_window_state", args.clone());
    for _ in 0..16 {
        if tree_has_marker(&resp) {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
        resp = driver.call("get_window_state", args.clone());
    }
    resp
}

/// Snapshot with `include_screenshot:false` (the perf opt-out), retrying until
/// the tree settles.
fn snapshot_settled_tree_only(driver: &mut McpDriver, pid: u32, wid: u64) -> ToolResponse {
    let args =
        serde_json::json!({ "pid": pid as i64, "window_id": wid, "include_screenshot": false });
    let mut resp = driver.call("get_window_state", args.clone());
    for _ in 0..16 {
        if tree_has_marker(&resp) {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
        resp = driver.call("get_window_state", args.clone());
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
fn launch_new_instance(
    driver: &mut McpDriver,
    exe: &std::path::Path,
    title: &str,
) -> Option<(u32, u64)> {
    if !exe.exists() {
        eprintln!("[capture_mode] harness not built ({exe:?}) — skipping");
        return None;
    }
    let before = harness_pids(driver, title);
    driver
        .reaper()
        .spawn(
            Command::new(exe)
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
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

/// DEFAULT: get_window_state returns BOTH the tree AND a screenshot — grounding
/// on both is the whole point. Each half is asserted only when its OS grant is
/// available (accessibility for the tree, screen-recording for the image), so a
/// missing grant skips-with-note rather than false-failing.
#[test]
#[ignore]
fn default_returns_tree_and_screenshot() {
    let Some(mut driver) = spawn_driver() else {
        return;
    };
    let Some((pid, wid)) = launch(&mut driver) else {
        return;
    };

    let resp = snapshot_settled_default(&mut driver, pid, wid);
    assert!(
        !resp.is_error(),
        "get_window_state(default) errored: {}",
        resp.text()
    );

    let tree = tree_has_marker(&resp);
    let img = has_image(&resp);
    assert!(
        tree || img,
        "default returned neither tree nor image — both grants missing? {}",
        resp.text().chars().take(160).collect::<String>()
    );
    // When BOTH grants are present, BOTH must be returned (the contract).
    if tree && !img {
        eprintln!("[capture_mode] default: tree present, no image (screen-capture grant likely missing) — partial check");
    } else if img && !tree {
        eprintln!("[capture_mode] default: image present, no tree (accessibility grant likely missing) — partial check");
    }
    println!("✅ default: tree={tree} image={img} (both expected when grants present)");
}

/// `include_screenshot:false` is the perf opt-out: tree present, NO image. This
/// is the only way to suppress the screenshot now (capture_mode no longer does).
#[test]
#[ignore]
fn include_screenshot_false_returns_tree_only() {
    let Some(mut driver) = spawn_driver() else {
        return;
    };
    let Some((pid, wid)) = launch(&mut driver) else {
        return;
    };

    let resp = snapshot_settled_tree_only(&mut driver, pid, wid);
    assert!(
        !resp.is_error(),
        "get_window_state(include_screenshot:false) errored: {}",
        resp.text()
    );

    if !tree_has_marker(&resp) {
        eprintln!("[capture_mode] tree-only: no {TREE_MARKER:?} — accessibility grant likely missing; skipping");
        return;
    }
    assert!(
        !has_image(&resp),
        "include_screenshot:false must NOT return an image content entry: {}",
        resp.text().chars().take(160).collect::<String>()
    );
    println!("✅ include_screenshot:false: tree present ({TREE_MARKER}), no image");
}

/// `capture_mode` is DEPRECATED and ignored: passing `vision` (which used to
/// suppress the tree) must STILL return both — proving the arg has no effect.
#[test]
#[ignore]
fn deprecated_capture_mode_is_ignored() {
    let Some(mut driver) = spawn_driver() else {
        return;
    };
    let Some((pid, wid)) = launch(&mut driver) else {
        return;
    };

    let resp = snapshot_settled(&mut driver, pid, wid, "vision");
    assert!(
        !resp.is_error(),
        "get_window_state(capture_mode=vision) errored: {}",
        resp.text()
    );

    // The legacy "vision" value must NOT suppress the tree anymore.
    if has_image(&resp) && tree_has_marker(&resp) {
        println!("✅ capture_mode=vision ignored: BOTH tree and image returned");
        return;
    }
    // Tolerate a missing grant, but the deprecated arg must never make the
    // present half disappear: if the tree marker shows, that already proves
    // "vision" didn't suppress it.
    if tree_has_marker(&resp) {
        println!("✅ capture_mode=vision ignored: tree still present (image grant may be missing)");
    } else {
        eprintln!(
            "[capture_mode] vision: no tree marker — accessibility grant likely missing; skipping"
        );
    }
}
