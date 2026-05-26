//! Integration test against the CuaTestHarness.SwiftUI Swift app.
//!
//! macOS equivalent of `harness_winui3_test.rs` — SwiftUI plays the same
//! role on macOS that WinUI3 plays on Windows: the modern declarative
//! UI hosting pattern with retained-mode AX exposed via a different
//! backend than the older AppKit one.
//!
//! Scenarios (see scenarios.json `swiftui` section):
//!   - counter   : SwiftUI Button increments State<Int>
//!   - text_body : Text with HARNESS_TEXT_MARKER_v1
//!   - text_input: TextField with mirror Text
//!   - popover   : .popover() — SwiftUI analogue of WinUI3 CommandBarFlyout
//!
//! Run locally (after `libs/cua-driver/test-harness/build.sh`):
//!   cargo test --test harness_swiftui_test -- --ignored --nocapture

#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

fn workspace_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).parent().unwrap().parent().unwrap().to_owned()
}

fn driver_binary() -> PathBuf {
    let root = workspace_root();
    let release = root.join("target/release/cua-driver");
    if release.exists() { return release; }
    root.join("target/debug/cua-driver")
}

fn harness_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_SWIFTUI_APP") {
        let pb = PathBuf::from(p).join("Contents/MacOS/CuaTestHarness.SwiftUI");
        if pb.exists() { return pb; }
    }
    workspace_root().join(
        "test-apps/harness-swiftui/CuaTestHarness.SwiftUI.app/Contents/MacOS/CuaTestHarness.SwiftUI")
}

fn send(stdin: &mut ChildStdin, req: serde_json::Value) {
    writeln!(stdin, "{}", serde_json::to_string(&req).unwrap()).unwrap();
}

fn recv(stdout: &mut BufReader<&mut ChildStdout>) -> serde_json::Value {
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    serde_json::from_str(line.trim()).expect("parse json")
}

fn init(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>) {
    send(stdin, serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
    }));
    let _ = recv(stdout);
}

fn tools_call(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
              id: u32, name: &str, args: serde_json::Value) -> serde_json::Value {
    send(stdin, serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": name, "arguments": args }
    }));
    recv(stdout)
}

struct Harness { _app: Child, pid: u32 }

impl Harness {
    fn launch() -> Option<Self> {
        let exe = harness_exe();
        if !exe.exists() {
            eprintln!("harness exe not found at {exe:?}");
            return None;
        }
        let app = Command::new(&exe)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().ok()?;
        let pid = app.id();
        std::thread::sleep(Duration::from_millis(900));
        Some(Self { _app: app, pid })
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self._app.kill();
        let _ = self._app.wait();
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn find_harness_window(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
                       pid: u32, title_substr: &str) -> Option<(u64, String)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    let mut id = 10u32;
    loop {
        let resp = tools_call(stdin, stdout, id, "list_windows",
            serde_json::json!({ "pid": pid as i64 }));
        id = id.wrapping_add(1);
        if let Some(wins) = resp["result"]["structuredContent"]["windows"].as_array() {
            for w in wins {
                if w["pid"].as_u64() != Some(pid as u64) { continue; }
                let title = w["title"].as_str().unwrap_or("");
                if title.contains(title_substr) {
                    return Some((w["window_id"].as_u64()?, title.to_string()));
                }
            }
        }
        if std::time::Instant::now() >= deadline { return None; }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn snapshot_elements(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
                     pid: u32, window_id: u64) -> serde_json::Value {
    tools_call(stdin, stdout, 20, "get_window_state",
        serde_json::json!({
            "pid": pid as i64,
            "window_id": window_id,
            "capture_mode": "tree"
        }))
}

fn snapshot_text(snap: &serde_json::Value) -> &str {
    snap["result"]["content"][0]["text"].as_str().unwrap_or("")
}

fn elements_have_aid(snap: &serde_json::Value, aid: &str) -> bool {
    snapshot_text(snap).contains(&format!("id={aid}"))
}

fn find_element_index_by_aid(snap: &serde_json::Value, aid: &str) -> Option<u64> {
    let needle = format!("id={aid}");
    for line in snapshot_text(snap).lines() {
        if !line.contains(&needle) { continue; }
        let start = line.find('[')? + 1;
        let end = line[start..].find(']')? + start;
        return line[start..end].trim().parse().ok();
    }
    None
}

fn ax_tree_looks_empty(snap: &serde_json::Value) -> bool {
    let txt = snapshot_text(snap);
    let line_count = txt.lines().count();
    txt.is_empty() || line_count <= 2 ||
        txt.contains("Accessibility permission required") ||
        txt.contains("TCC permission")
}

#[test]
#[ignore]
fn harness_swiftui_smoke() {
    let driver = driver_binary();
    if !driver.exists() { eprintln!("cua-driver not built"); return; }
    let harness = match Harness::launch() {
        Some(h) => h,
        None => { eprintln!("harness not built — skipping"); return; }
    };
    println!("harness pid={}", harness.pid);

    let mut child = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);

    let (wid, title) = find_harness_window(&mut stdin, &mut stdout, harness.pid,
                                           "CuaTestHarness SwiftUI")
        .expect("main window not found");
    println!("main window: id={wid} title={title:?}");

    let snap = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    if ax_tree_looks_empty(&snap) {
        eprintln!("AX empty — TCC not granted; skipping element-assertions");
        let _ = child.kill(); return;
    }
    let text = snapshot_text(&snap);
    println!("snapshot:\n{text}");

    // SwiftUI Text views render as AXStaticText leaves and don't propagate
    // accessibilityIdentifier into the AX tree's identifier slot (same
    // quirk as AppKit's NSTextField label mode + WPF's TextBlock). Assert
    // on text content for labels, AX-id only for actionable controls.
    for aid in [
        "btn-increment", "btn-reset",
        "txt-input",
        "btn-open-popover",
        "btn-exit",
    ] {
        assert!(elements_have_aid(&snap, aid),
                "missing AX identifier {aid} in SwiftUI snapshot");
    }

    assert!(text.contains("HARNESS_TEXT_MARKER_v1"),
            "text_body marker not in SwiftUI snapshot");

    let _ = child.kill();
}

/// popover: click the popover trigger, verify the popover body text appears
/// in the AX tree after the open. SwiftUI's analogue of WinUI3 CommandBarFlyout.
#[test]
#[ignore]
fn harness_swiftui_popover() {
    let driver = driver_binary();
    if !driver.exists() { eprintln!("cua-driver not built"); return; }
    let harness = match Harness::launch() {
        Some(h) => h,
        None => { eprintln!("harness not built — skipping"); return; }
    };

    let mut child = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);

    let (wid, _) = find_harness_window(&mut stdin, &mut stdout, harness.pid,
                                       "CuaTestHarness SwiftUI")
        .expect("main window not found");
    let snap_pre = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    if ax_tree_looks_empty(&snap_pre) {
        eprintln!("AX empty — TCC not granted; skipping"); let _ = child.kill(); return;
    }
    // Verify popover body is NOT yet in the tree.
    let pre_text = snapshot_text(&snap_pre).to_owned();
    assert!(!pre_text.contains("POPOVER_MARKER_v1"),
            "popover body unexpectedly present BEFORE open");

    let trigger_idx: u64 = if let Some(i) = find_element_index_by_aid(&snap_pre, "btn-open-popover") {
        i
    } else {
        eprintln!("popover trigger not found, skipping");
        let _ = child.kill();
        return;
    };
    let click = tools_call(&mut stdin, &mut stdout, 50, "click",
        serde_json::json!({
            "pid": harness.pid as i64,
            "window_id": wid,
            "element_index": trigger_idx,
            "action": "press"
        }));
    println!("popover trigger click: {click}");

    std::thread::sleep(Duration::from_millis(300));

    // Popovers may live in a separate AXWindow on macOS — try the main
    // window first, then list_windows for additional candidates.
    let snap_post = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    let mut found_marker = snapshot_text(&snap_post).contains("POPOVER_MARKER_v1");
    if !found_marker {
        // Walk any new windows for the same pid.
        let resp = tools_call(&mut stdin, &mut stdout, 51, "list_windows",
            serde_json::json!({ "pid": harness.pid as i64 }));
        if let Some(wins) = resp["result"]["structuredContent"]["windows"].as_array() {
            for w in wins {
                if let Some(other_wid) = w["window_id"].as_u64() {
                    if other_wid == wid { continue; }
                    let s = snapshot_elements(&mut stdin, &mut stdout, harness.pid, other_wid);
                    if snapshot_text(&s).contains("POPOVER_MARKER_v1") {
                        found_marker = true; break;
                    }
                }
            }
        }
    }
    assert!(found_marker, "popover body marker not found after open");

    let _ = child.kill();
}
