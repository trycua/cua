//! Integration test against the CuaTestHarness.WinUI3 .NET 8 app.
//!
//! Pair-companion to harness_wpf_test.rs but exercises WinUI3 + DComp
//! rendering paths. The two tests share scenario IDs (counter, text_body,
//! exit) so any AutomationId regression that affects both UI frameworks
//! shows up in both suites.
//!
//! WinUI3-specific surfaces under test:
//!   - CommandBarFlyout : popup hosted in same HWND, rendered via XAML
//!                        Islands / DComp. Tests UIA descent into the
//!                        flyout subtree.
//!   - XAML Popup       : Popup primitive (NOT a separate HWND).
//!                        Regression guard that the agent doesn't lose
//!                        track of in-window flyouts.
//!
//! Run via:
//!   .\sandbox\run-tests-in-sandbox.ps1 harness_winui3
//! or locally:
//!   cargo test --test harness_winui3_test -- --ignored --nocapture

#![cfg(target_os = "windows")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

fn workspace_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).parent().unwrap().parent().unwrap().to_owned()
}

fn driver_binary() -> PathBuf { workspace_root().join("target/debug/cua-driver.exe") }

fn harness_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_WINUI3_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() { return pb; }
    }
    workspace_root().join("test-apps/harness-winui3/CuaTestHarness.WinUI3.exe")
}

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

fn tools_call(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
              id: u32, name: &str, args: serde_json::Value) -> serde_json::Value {
    send(stdin, serde_json::json!({
        "jsonrpc":"2.0","id":id,"method":"tools/call",
        "params":{"name":name,"arguments":args}
    }));
    recv(stdout)
}

fn snapshot_text(snapshot: &serde_json::Value) -> &str {
    snapshot["result"]["content"][0]["text"].as_str().unwrap_or("")
}

struct Harness { _app: Child, pid: u32 }

impl Harness {
    fn launch() -> Option<Self> {
        let exe = harness_exe();
        if !exe.exists() {
            eprintln!("WinUI3 harness exe not found at {exe:?} — run test-harness/build.ps1");
            return None;
        }
        let app = Command::new(&exe)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().ok()?;
        let pid = app.id();
        // Short fixed cold-start settle (window creation + foreground
        // establishment after spawn). Polling in find_harness_window
        // handles the variable tail.
        std::thread::sleep(Duration::from_millis(1500));
        Some(Self { _app: app, pid })
    }
}

impl Drop for Harness {
    fn drop(&mut self) { let _ = self._app.kill(); }
}

fn find_harness_window(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
                       pid: u32, title_substr: &str) -> Option<(u64, String)> {
    // Polls because WinUI3 cold-start (first run, no cached WinAppSDK) can
    // exceed a fixed 5s wait under sandbox load. Bounded so a genuinely
    // broken harness still fails the test in ≤20s rather than hanging.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
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
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
#[ignore]
fn harness_winui3_smoke() {
    let driver = driver_binary();
    if !driver.exists() { eprintln!("cua-driver.exe not built"); return; }
    let harness = match Harness::launch() { Some(h) => h, None => return };
    println!("WinUI3 harness pid={}", harness.pid);

    let mut child = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);

    let (wid, _) = find_harness_window(&mut stdin, &mut stdout, harness.pid, "CuaTestHarness WinUI3")
        .expect("WinUI3 main window not found");

    let snap = tools_call(&mut stdin, &mut stdout, 20, "get_window_state",
        serde_json::json!({"pid": harness.pid as i64, "window_id": wid, "capture_mode":"tree"}));
    let text = snapshot_text(&snap);

    // Button-class controls surface AutomationIds in the UIA tree.
    for aid in [
        "btn-increment", "btn-reset",
        "btn-open-flyout",
        "btn-open-popup",
        "btn-exit",
    ] {
        assert!(text.contains(&format!("id={aid}")),
            "missing AutomationId {aid} in WinUI3 UIA snapshot");
    }

    // TextBlock content (no AutomationId surfaces) — assert markers.
    assert!(text.contains("HARNESS_TEXT_MARKER_v1"), "WinUI3 text_body marker not in snapshot");
    assert!(text.contains("counter=0"),             "WinUI3 initial counter label not in snapshot");

    println!("✅ harness_winui3_smoke: all expected scenarios present in UIA tree");

    child.kill().ok();
}

fn find_idx(text: &str, aid: &str) -> Option<u64> {
    let needle = format!("id={aid}");
    for line in text.lines() {
        if !line.contains(&needle) { continue; }
        let s = line.find('[')? + 1;
        let e = line[s..].find(']')? + s;
        return line[s..e].trim().parse().ok();
    }
    None
}

#[test]
#[ignore]
fn harness_winui3_type_text() {
    let driver = driver_binary();
    if !driver.exists() { return; }
    let harness = match Harness::launch() { Some(h) => h, None => return };

    let mut child = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);

    let (wid, _) = find_harness_window(&mut stdin, &mut stdout, harness.pid, "CuaTestHarness WinUI3")
        .expect("WinUI3 main window");

    let snap = tools_call(&mut stdin, &mut stdout, 20, "get_window_state",
        serde_json::json!({"pid": harness.pid as i64, "window_id": wid, "capture_mode":"som"}));
    let snap_text = snapshot_text(&snap);
    let idx = find_idx(snap_text, "txt-input").expect("txt-input not in WinUI3 snapshot");

    // WinUI3 is a XAML host — type_text requires element_index + window_id
    // (routes through UIA ValuePattern.SetValue, see Windows backend docs).
    let resp = tools_call(&mut stdin, &mut stdout, 30, "type_text", serde_json::json!({
        "pid": harness.pid as i64,
        "window_id": wid,
        "element_index": idx,
        "text": "winui3-typed"
    }));
    println!("type_text (WinUI3): {}", resp["result"]["content"][0]["text"]);
    std::thread::sleep(Duration::from_millis(500));

    let post = tools_call(&mut stdin, &mut stdout, 31, "get_window_state",
        serde_json::json!({"pid": harness.pid as i64, "window_id": wid, "capture_mode":"som"}));
    let post_text = snapshot_text(&post);
    assert!(post_text.contains("mirror=winui3-typed"),
        "WinUI3 TextBox mirror did not advance. Snapshot excerpt: {}",
        post_text.chars().take(600).collect::<String>());
    println!("✅ harness_winui3_type_text: WinUI3 TextBox mirror reflects 'winui3-typed'");

    child.kill().ok();
}

#[test]
#[ignore]
fn harness_winui3_xaml_popup_open() {
    let driver = driver_binary();
    if !driver.exists() { return; }
    let harness = match Harness::launch() { Some(h) => h, None => return };

    let mut child = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);

    let (wid, _) = find_harness_window(&mut stdin, &mut stdout, harness.pid, "CuaTestHarness WinUI3")
        .expect("WinUI3 main window");

    let snap = tools_call(&mut stdin, &mut stdout, 20, "get_window_state",
        serde_json::json!({"pid": harness.pid as i64, "window_id": wid, "capture_mode":"som"}));
    let idx = find_idx(snapshot_text(&snap), "btn-open-popup").expect("btn-open-popup");

    let _ = tools_call(&mut stdin, &mut stdout, 30, "click", serde_json::json!({
        "pid": harness.pid as i64, "window_id": wid, "element_index": idx
    }));
    std::thread::sleep(Duration::from_millis(500));

    let post = tools_call(&mut stdin, &mut stdout, 31, "get_window_state",
        serde_json::json!({"pid": harness.pid as i64, "window_id": wid, "capture_mode":"som"}));
    let text = snapshot_text(&post);
    assert!(text.contains("XAML_POPUP_MARKER_v1"),
        "XAML popup body did not appear in tree after click. Excerpt: {}",
        text.chars().take(600).collect::<String>());
    println!("✅ harness_winui3_xaml_popup_open: popup body visible in UIA tree");

    child.kill().ok();
}

// ── Session helper for the additional control tests ──────────────────────────

fn winui3_with_session<F>(f: F)
where F: FnOnce(u32, u64, &mut ChildStdin, &mut BufReader<&mut ChildStdout>) {
    let driver = driver_binary();
    if !driver.exists() { eprintln!("cua-driver.exe not built"); return; }
    let harness = match Harness::launch() { Some(h) => h, None => return };
    let mut child = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);
    let (wid, _) = find_harness_window(&mut stdin, &mut stdout, harness.pid, "CuaTestHarness WinUI3")
        .expect("WinUI3 main window");
    f(harness.pid, wid, &mut stdin, &mut stdout);
    drop(stdout);
    drop(stdin);
    child.kill().ok();
}

/// Documents the WinUI3 CheckBox gap. cua-driver `click` tries UIA Invoke
/// (CheckBox has TogglePattern only, no InvokePattern), then falls
/// through to PostMessage WM_LBUTTONDOWN/UP. PostMessage doesn't reach
/// the WinUI3 input chain (the CoreInput dispatcher only consumes events
/// from the system input queue — same reason type_text on XAML hosts
/// requires UIA ValuePattern). Real fix: cua-driver should try
/// TogglePatternId.Toggle() before falling through to PostMessage on
/// XAML hosts.
#[test]
#[ignore]
fn harness_winui3_checkbox_toggle_DOCUMENTED_no_op() {
    winui3_with_session(|pid, wid, stdin, stdout| {
        let snap = tools_call(stdin, stdout, 20, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let idx = find_idx(snapshot_text(&snap), "chk-agreed").expect("chk-agreed");
        let _ = tools_call(stdin, stdout, 30, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx
        }));
        std::thread::sleep(Duration::from_millis(400));
        let post = tools_call(stdin, stdout, 31, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        assert!(snapshot_text(&post).contains("agreed=False"),
            "Expected WinUI3 CheckBox no-op (toggle pattern not attempted). \
             If now agreed=True, cua-driver added TogglePattern dispatch.");
        println!("⚠️  harness_winui3_checkbox_toggle_DOCUMENTED_no_op: toggle not dispatched");
    });
}

/// Documents cua-driver gap: the `click` tool tries UIA Invoke then falls
/// back to PostMessage. WinUI3 RadioButton implements
/// `SelectionItemPattern.Select` (not Invoke), and PostMessage clicks
/// don't reach its handler chain. Real fix: cua-driver should detect
/// `SelectionItemPattern` on the target and call `Select()` as one of
/// the pattern attempts before falling through to PostMessage.
#[test]
#[ignore]
fn harness_winui3_radio_select_DOCUMENTED_no_op() {
    winui3_with_session(|pid, wid, stdin, stdout| {
        let snap = tools_call(stdin, stdout, 20, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let idx = find_idx(snapshot_text(&snap), "rdo-high").expect("rdo-high");
        let _ = tools_call(stdin, stdout, 30, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx
        }));
        std::thread::sleep(Duration::from_millis(400));
        let post = tools_call(stdin, stdout, 31, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let text = snapshot_text(&post);
        // Expected behaviour today: state stays at Low (the click was a no-op).
        // When cua-driver adds SelectionItemPattern.Select dispatch, flip this
        // assertion to assert prio=High and rename to `_radio_select`.
        assert!(text.contains("prio=Low"),
            "Expected the documented WinUI3 RadioButton no-op (prio stays Low). \
             If this now asserts prio=High, cua-driver added SelectionItemPattern \
             support — update the test.");
        println!("⚠️  harness_winui3_radio_select_DOCUMENTED_no_op: confirmed no-op (UIA Invoke fell through, SelectionItem.Select not attempted)");
    });
}

/// Documents cua-driver gap: WinUI3 Slider implements
/// `RangeValuePattern`, not `ValuePattern`. cua-driver's `set_value` tool
/// queries `ValuePatternId` specifically (impl_.rs:2640), so it silently
/// fails on RangeValuePattern-only elements. Real fix: cua-driver should
/// try RangeValuePattern.SetValue (coercing the string to a double) when
/// ValuePattern isn't found.
/// WinUI3 Slider's AutomationId doesn't surface in the flat UIA element
/// list (same quirk as WPF Slider — SliderAutomationPeer doesn't show up
/// as an indexed actionable element). Slider sub-parts (Decrease/Increase
/// thumb) similarly aren't exposed in WinUI3's tree. Together with the
/// `set_value` tool only trying ValuePatternId (not RangeValuePattern),
/// driving a WinUI3 Slider via cua-driver isn't currently possible.
/// Real fix: enumerate Slider's sub-parts in UIA + `set_value` tries
/// RangeValuePattern when ValuePattern isn't supported.
#[test]
#[ignore]
fn harness_winui3_slider_DOCUMENTED_unreachable() {
    winui3_with_session(|pid, wid, stdin, stdout| {
        let snap = tools_call(stdin, stdout, 20, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let idx_opt = find_idx(snapshot_text(&snap), "sld-value");
        assert!(idx_opt.is_none(),
            "WinUI3 Slider's AutomationId 'sld-value' now appears in the UIA tree — \
             cua-driver may have fixed the slider-element enumeration gap.");
        println!("⚠️  harness_winui3_slider_DOCUMENTED_unreachable: sld-value not in UIA flat tree");
    });
}

/// WinUI3 ComboBox uses ExpandCollapsePattern for the parent and
/// SelectionItemPattern for items. cua-driver's `click` tool tries
/// InvokePattern first, then PostMessage — neither fires WinUI3's
/// ComboBox handlers reliably. This test documents the gap; a real fix
/// would have `click` try ExpandCollapse on parents and SelectionItem
/// on items before falling through to PostMessage.
#[test]
#[ignore]
fn harness_winui3_combo_select_DOCUMENTED_no_op() {
    winui3_with_session(|pid, wid, stdin, stdout| {
        let snap = tools_call(stdin, stdout, 20, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let combo_idx = find_idx(snapshot_text(&snap), "cbo-color").expect("cbo-color");
        let _ = tools_call(stdin, stdout, 30, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": combo_idx
        }));
        std::thread::sleep(Duration::from_millis(400));
        let post = tools_call(stdin, stdout, 31, "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        assert!(snapshot_text(&post).contains("color=green"),
            "Expected WinUI3 ComboBox to stay at default green (Invoke fell through, \
             ExpandCollapse not attempted). If now color=orange, cua-driver added \
             ExpandCollapse dispatch.");
        println!("⚠️  harness_winui3_combo_select_DOCUMENTED_no_op: ExpandCollapse not dispatched");
    });
}
