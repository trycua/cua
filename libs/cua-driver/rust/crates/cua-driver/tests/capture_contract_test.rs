//! `get_window_state` perception contract,
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
//! Run explicitly:
//!   cargo test -p cua-driver --test capture_contract_test -- --ignored --nocapture --test-threads=1

#![cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::{harness_app, Driver, McpDriver, ToolResponse};

/// The aid every harness exposes on its increment button — the tree marker
/// (WPF AutomationId / AppKit AX identifier / GTK3 AT-SPI accessible name).
const TREE_MARKER: &str = "btn-increment";

fn strict() -> bool {
    std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some()
}

#[cfg(target_os = "macos")]
fn spawn_driver() -> Option<McpDriver> {
    McpDriver::spawn_macos_daemon_proxy()
}

#[cfg(not(target_os = "macos"))]
fn spawn_driver() -> Option<McpDriver> {
    McpDriver::spawn()
}

fn test_driver() -> Option<McpDriver> {
    let driver = spawn_driver();
    assert!(
        driver.is_some() || !strict(),
        "required source-built driver did not start"
    );
    driver
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
        if strict() {
            panic!("required capture harness is missing: {exe:?}");
        }
        eprintln!("[capture] optional harness not built: {exe:?}");
        return None;
    }
    let before = harness_pids(driver, title);
    let spawned = driver
        .reaper()
        .spawn(
            Command::new(exe)
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        );
    if let Err(error) = spawned {
        if strict() {
            panic!("failed to launch required capture harness {exe:?}: {error}");
        }
        return None;
    }
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
    if strict() {
        panic!("required capture harness window {title:?} never appeared");
    }
    eprintln!("[capture] optional harness window {title:?} never appeared");
    None
}

// ── tests ───────────────────────────────────────────────────────────────────────

/// DEFAULT: get_window_state returns BOTH the tree AND a screenshot — grounding
/// on both is the whole point.
#[test]
#[ignore]
fn default_returns_tree_and_screenshot() {
    let Some(mut driver) = test_driver() else {
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

    assert!(
        tree_has_marker(&resp),
        "default response is missing the required tree marker: {}",
        resp.text().chars().take(160).collect::<String>()
    );
    assert!(has_image(&resp), "default response is missing its screenshot");
    println!("capture default: tree and image present");
}

/// `include_screenshot:false` is the perf opt-out: tree present, NO image. This
/// is the only way to suppress the screenshot now (capture_mode no longer does).
#[test]
#[ignore]
fn include_screenshot_false_returns_tree_only() {
    let Some(mut driver) = test_driver() else {
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

    assert!(
        tree_has_marker(&resp),
        "tree-only response is missing {TREE_MARKER:?}"
    );
    assert!(
        !has_image(&resp),
        "include_screenshot:false must NOT return an image content entry: {}",
        resp.text().chars().take(160).collect::<String>()
    );
    println!("capture tree-only: marker present and image absent");
}

/// `capture_mode` is DEPRECATED and ignored: passing `vision` (which used to
/// suppress the tree) must STILL return both — proving the arg has no effect.
#[test]
#[ignore]
fn deprecated_capture_mode_is_ignored() {
    let Some(mut driver) = test_driver() else {
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

    assert!(
        tree_has_marker(&resp),
        "deprecated capture_mode=vision suppressed the tree"
    );
    assert!(
        has_image(&resp),
        "deprecated capture_mode=vision suppressed the screenshot"
    );
    println!("capture_mode=vision ignored: tree and image returned");
}
