//! Integration test against the CuaTestHarness.Wpf .NET 8 app.
//!
//! Pairs of test apps live under `libs/cua-driver/test-harness/`. They
//! publish into `libs/cua-driver/rust/test-apps/harness-{wpf,winui3}/` and
//! get mapped into the sandbox via the existing run-tests-in-sandbox.ps1
//! mapped folder.
//!
//! Each scenario covers a Win32 hosting pattern the agent should handle:
//!   - counter        : UIA Invoke on a plain WPF button
//!   - text_body      : get_window_state extracts known marker text
//!   - message_box    : modal MessageBox enumeration
//!   - bottom_strip   : Save/Cancel buttons present in the UIA tree
//!                      (regression guard for the GetClientRect-vs-
//!                      GetWindowRect capture bug fixed in #1696)
//!   - owned_popup    : owned secondary window discovered via list_windows
//!   - layered_popup  : WS_EX_LAYERED window enumerated and captured
//!   - child_hwnd     : native Win32 BUTTON child HWND visible in tree
//!
//! Run via the sandbox runner:
//!   .\sandbox\run-tests-in-sandbox.ps1 harness_wpf
//!
//! Or locally (requires .NET 8 SDK + `test-harness/build/windows.ps1`):
//!   cargo test --test harness_wpf_test -- --ignored --nocapture
//!
//! Tests are `#[ignore]` so they don't run in plain `cargo test`. The
//! sandbox runner unignores them explicitly via the `--ignored` arg.
//!
//! **Foreground-lock caveat:** a handful of these tests (`double_click`,
//! `right_click`, `type_text`) rely on `dispatch:"foreground"` to reach
//! WPF's input chain reliably. Windows' system-wide foreground-lock
//! kicks in after ~30s with no real user input — once that happens,
//! `SetForegroundWindow` is denied for non-UIAccess processes and the
//! tests fail. Run the WPF and WinUI3 suites as **separate** `cargo test`
//! invocations (`--test harness_wpf_test`, then `--test harness_winui3_test`)
//! rather than combined: each binary's first ~30s falls inside the
//! foreground-lock window and stays green. The sandbox runner does this
//! automatically.

#![cfg(target_os = "windows")]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::{ax, harness_app, spawn_in_job, Driver, McpDriver, ToolResponse};

// ── harness launcher ─────────────────────────────────────────────────────────

fn harness_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_WPF_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() { return pb; }
    }
    harness_app("harness-wpf", "CuaTestHarness.Wpf.exe")
}

/// Launch the WPF harness app, tying its lifetime to `driver`'s reaper (the
/// Windows kill-on-close Job Object) and returning its pid. Returns `None`
/// (with a skip message) if the harness isn't built. We spawn via
/// `spawn_in_job` and grab the pid before handing the child to the reaper so
/// every subsequent `list_windows`/`find_window` filters on the exact spawned
/// pid — defensive against the corner case where a previous test's harness
/// hasn't been fully reaped and there are briefly two CuaTestHarness.Wpf
/// windows on the desktop.
fn launch_harness(driver: &mut McpDriver) -> Option<u32> {
    let exe = harness_exe();
    if !exe.exists() {
        eprintln!("harness exe not found at {exe:?} — run test-harness/build/windows.ps1 first");
        return None;
    }
    let app = spawn_in_job(
        Command::new(&exe).stdout(Stdio::null()).stderr(Stdio::null()),
    ).ok()?;
    let pid = app.id();
    driver.reaper().push(app);
    // Short fixed settle for cold-start (window-creation + initial
    // foreground hand-off after spawn) — the rest of the readiness
    // wait happens via polling in find_window. A 200ms-only
    // wait turned out to be too short for the harness to establish
    // foreground reliably under test-batch load, which caused
    // SetForegroundWindow-needing tests (dispatch:foreground) to
    // fail with a foreground-lock rejection.
    std::thread::sleep(Duration::from_millis(800));
    Some(pid)
}

// Get a UIA snapshot's flat element list — used to assert AutomationIds
// exist in the tree without depending on tree-walk order, and to read the
// harness's marker/status text.
fn snapshot(driver: &mut McpDriver, pid: u32, window_id: u64) -> ToolResponse {
    driver.call("get_window_state", serde_json::json!({
        "pid": pid as i64,
        "window_id": window_id,
        "capture_mode": "tree"
    }))
}

// ── tests ────────────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn harness_wpf_smoke() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some(pid) = launch_harness(&mut driver) else { return };
    println!("harness pid={}", pid);

    let (wid, title) = driver.find_window(pid as i64, "CuaTestHarness WPF")
        .expect("main window not found via list_windows");
    println!("main window: id={} title={:?}", wid, title);

    let snap = snapshot(&mut driver, pid, wid);
    let text = snap.text();

    // Buttons appear with explicit id=<aid> tags in the UIA markdown.
    for aid in [
        "btn-increment", "btn-reset",
        "btn-open-msgbox",
        "btn-save", "btn-cancel",                       // regression guard for #1696
        "btn-open-owned", "btn-open-layered",
        "btn-exit",
    ] {
        assert!(ax::has_id(text, aid), "missing AutomationId {aid} in WPF UIA snapshot");
    }

    // TextBlocks are reported as bare Text nodes (no UIA Invoke/Value pattern,
    // no AutomationId in the rendered tree). Assert on their content instead.
    assert!(text.contains("HARNESS_TEXT_MARKER_v1"), "text_body marker not in snapshot");
    assert!(text.contains("counter=0"),             "initial counter label not in snapshot");
    assert!(text.contains("accel_fired=0"),         "initial accel label not in snapshot");

    // HwndHost child should surface the native Win32 BUTTON as a UIA Button.
    assert!(text.contains("\"Native Win32 Child\""), "native HWND child button not in snapshot");

    println!("✅ harness_wpf_smoke: all expected scenarios present in UIA tree");
}

// ── shared driver session helper ─────────────────────────────────────────────

/// Spin up a fresh cua-driver + harness pair, run the closure, then tear
/// everything down (the harness app is reaped with the driver via the Job
/// Object). Returns whatever the closure returns. The closure receives the
/// harness pid, a pre-resolved main window_id, and the driver.
fn with_session<F, R>(f: F) -> Option<R>
where F: FnOnce(u32, u64, &mut McpDriver) -> R {
    let mut driver = McpDriver::spawn()?;
    let pid = launch_harness(&mut driver)?;
    let (wid, _) = driver.find_window(pid as i64, "CuaTestHarness WPF")
        .expect("main window");
    Some(f(pid, wid, &mut driver))
}

#[test]
#[ignore]
fn harness_wpf_counter_invoke() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some(pid) = launch_harness(&mut driver) else { return };

    let (wid, _) = driver.find_window(pid as i64, "CuaTestHarness WPF")
        .expect("main window");
    // Pre-snapshot so element_cache has indices we can address.
    let pre = snapshot(&mut driver, pid, wid);
    let idx = ax::element_index_by_id(pre.text(), "btn-increment")
        .expect("btn-increment not in pre-snapshot");

    let click = driver.call("click", serde_json::json!({
        "pid": pid as i64,
        "window_id": wid,
        "element_index": idx
    }));
    println!("click [{idx}] btn-increment: {}", click.text());

    std::thread::sleep(Duration::from_millis(300));

    let post = snapshot(&mut driver, pid, wid);
    let text = post.text();
    assert!(
        text.contains("counter=1"),
        "counter label did not advance after click — snapshot text: {}",
        text.chars().take(400).collect::<String>()
    );
    println!("✅ harness_wpf_counter_invoke: counter advanced to 1");
}

#[test]
#[ignore]
fn harness_wpf_type_text() {
    with_session(|pid, wid, driver| {
        let snap = snapshot(driver, pid, wid);
        let idx = ax::element_index_by_id(snap.text(), "txt-input")
            .expect("txt-input not in snapshot");

        // WPF's TextBox needs *keyboard focus* for WM_CHAR delivery — and
        // PostMessage(WM_LBUTTONDOWN) doesn't reliably transfer keyboard
        // focus (WPF's input system treats posted events differently from
        // real ones). Use dispatch:"foreground" → SendInput synthesizes
        // an OS-level click that WPF treats identically to a user mouse,
        // landing actual keyboard focus on the TextBox.
        let _ = driver.call("bring_to_front", serde_json::json!({
            "pid": pid as i64, "window_id": wid
        }));
        std::thread::sleep(Duration::from_millis(300));

        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(400));

        // SendInput's restore_foreground_polling_best_effort may yank
        // foreground back from the harness window between click and
        // type_text. Re-assert foreground so PostMessage WM_CHAR finds
        // the TextBox with keyboard focus.
        let _ = driver.call("bring_to_front", serde_json::json!({
            "pid": pid as i64, "window_id": wid
        }));
        std::thread::sleep(Duration::from_millis(300));

        let resp = driver.call("type_text", serde_json::json!({
            "pid": pid as i64, "text": "harness-typed"
        }));
        println!("type_text: {}", resp.text());
        std::thread::sleep(Duration::from_millis(700));

        let post = snapshot(driver, pid, wid);
        let text = post.text();
        let mirror_lines: Vec<&str> = text.lines()
            .filter(|l| l.contains("mirror=") || l.contains("txt-input"))
            .collect();
        assert!(text.contains("mirror=harness-typed"),
            "TextBox mirror did not reflect typed text. Mirror/input lines: {:?}",
            mirror_lines);
        println!("✅ harness_wpf_type_text: TextBox mirror advanced to 'harness-typed'");
    });
}

#[test]
#[ignore]
fn harness_wpf_set_value() {
    // Companion to harness_wpf_type_text: exercises the UIA ValuePattern
    // write path via the `set_value` tool. No focus needed — purely UIA.
    with_session(|pid, wid, driver| {
        let snap = snapshot(driver, pid, wid);
        let idx = ax::element_index_by_id(snap.text(), "txt-input")
            .expect("txt-input not in snapshot");
        let _ = driver.call("set_value", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "value": "via-uia-setvalue"
        }));
        std::thread::sleep(Duration::from_millis(400));

        let post = snapshot(driver, pid, wid);
        let text = post.text();
        assert!(text.contains("mirror=via-uia-setvalue"),
            "set_value did not update TextBox. Excerpt: {}",
            text.chars().take(500).collect::<String>());
        println!("✅ harness_wpf_set_value: ValuePattern.SetValue wrote to TextBox");
    });
}

// In test-batch mode (many harnesses launched/killed in sequence) the WPF
// window's input pump occasionally misses background PostMessage events —
// reproducibly passes in isolation, intermittently fails in batch.
// `bring_to_front` pays the foreground swap once so the click test
// exercises the click-event-handling path itself, not the
// background-delivery path (which the counter_invoke test already
// covers via UIA Invoke).
fn focus_harness(driver: &mut McpDriver, pid: u32, wid: u64) {
    let _ = driver.call("bring_to_front", serde_json::json!({
        "pid": pid as i64, "window_id": wid
    }));
    std::thread::sleep(Duration::from_millis(300));
}

#[test]
#[ignore]
fn harness_wpf_right_click() {
    with_session(|pid, wid, driver| {
        focus_harness(driver, pid, wid);
        let snap = snapshot(driver, pid, wid);
        let idx = ax::element_index_by_id(snap.text(), "border-click-target")
            .expect("border-click-target not in snapshot");
        // Same dispatch:foreground rationale as type_text — PostMessage
        // WM_RBUTTONDOWN doesn't always reach WPF's MouseRightButtonDown
        // routed-event chain (intermittent in batch runs).
        let resp = driver.call("right_click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "dispatch": "foreground"
        }));
        println!("right_click: {}", resp.text());
        std::thread::sleep(Duration::from_millis(400));

        let post = snapshot(driver, pid, wid);
        let text = post.text();
        let action_lines: Vec<&str> = text.lines()
            .filter(|l| l.contains("last_action=") || l.contains("clicks="))
            .collect();
        assert!(text.contains("last_action=right_click"),
            "right_click handler did not fire. Action/click lines: {:?}", action_lines);
        println!("✅ harness_wpf_right_click: last_action=right_click");
    });
}

#[test]
#[ignore]
fn harness_wpf_double_click() {
    with_session(|pid, wid, driver| {
        focus_harness(driver, pid, wid);
        let snap = snapshot(driver, pid, wid);
        let idx = ax::element_index_by_id(snap.text(), "border-click-target")
            .expect("border-click-target not in snapshot");
        // dispatch:foreground for the same reason as right_click —
        // PostMessage WM_LBUTTONDOWN ×2 doesn't always reach WPF's
        // MouseDoubleClick / ClickCount=2 path under test-batch load.
        let resp = driver.call("double_click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "dispatch": "foreground"
        }));
        println!("double_click: {}", resp.text());
        std::thread::sleep(Duration::from_millis(400));

        let post = snapshot(driver, pid, wid);
        let text = post.text();
        let action_lines: Vec<&str> = text.lines()
            .filter(|l| l.contains("last_action=") || l.contains("clicks="))
            .collect();
        assert!(text.contains("last_action=double_click"),
            "double_click handler did not register a 2nd click. \
             Action/click lines: {:?}", action_lines);
        println!("✅ harness_wpf_double_click: last_action=double_click");
    });
}

#[test]
#[ignore]
fn harness_wpf_press_key_accelerator() {
    // F5 binding rather than the Ctrl+Shift+H one: cua-driver's hotkey
    // PostMessage path doesn't update OS modifier-key state (GetKeyState
    // returns "not pressed" for VK_CONTROL), so WPF's KeyBinding with
    // Modifiers=Control+Shift never matches. The UIA-worker SendInput
    // path would handle modifiers but requires the cua-driver-uia.exe
    // helper that isn't in our test config. F5 has no modifier and works
    // on the PostMessage path.
    with_session(|pid, wid, driver| {
        let resp = driver.call("press_key", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "key": "f5"
        }));
        println!("press_key f5: {}", resp.text());
        std::thread::sleep(Duration::from_millis(400));

        let post = snapshot(driver, pid, wid);
        let text = post.text();
        assert!(text.contains("accel_fired=1"),
            "F5 KeyBinding did not fire. Snapshot excerpt: {}",
            text.chars().take(500).collect::<String>());
        println!("✅ harness_wpf_press_key_accelerator: accel_fired=1 (F5 via PostMessage)");
    });
}

#[test]
#[ignore]
fn harness_wpf_scroll() {
    with_session(|pid, wid, driver| {
        // Pre-snapshot to populate the cache + read initial offset.
        let pre = snapshot(driver, pid, wid);
        let pre_text = pre.text();
        assert!(pre_text.contains("scroll_offset=0"),
            "expected initial scroll_offset=0, got: {}",
            pre_text.lines().filter(|l| l.contains("scroll_offset")).collect::<Vec<_>>().join(" / "));

        // Click into the ScrollViewer so it gets focus / its descendants
        // become the WM_VSCROLL target.
        let idx = ax::element_index_by_id(pre.text(), "scroll-tall")
            .expect("scroll-tall not in snapshot");
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx
        }));
        std::thread::sleep(Duration::from_millis(200));

        // Scroll down 5 lines.
        let resp = driver.call("scroll", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "direction": "down", "by": "line", "amount": 5
        }));
        println!("scroll down: {}", resp.text());
        std::thread::sleep(Duration::from_millis(400));

        let post = snapshot(driver, pid, wid);
        let text = post.text();
        let advanced = text.lines().any(|l|
            l.contains("scroll_offset=") && !l.contains("scroll_offset=0\""));
        assert!(advanced, "scroll offset did not advance after WM_VSCROLL. Lines: {}",
            text.lines().filter(|l| l.contains("scroll_offset"))
                .collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_scroll: scroll_offset advanced past 0");
    });
}

#[test]
#[ignore]
fn harness_wpf_modal_messagebox() {
    with_session(|pid, wid, driver| {
        let snap = snapshot(driver, pid, wid);
        let idx = ax::element_index_by_id(snap.text(), "btn-open-msgbox")
            .expect("btn-open-msgbox not in snapshot");
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx
        }));
        std::thread::sleep(Duration::from_millis(600));

        // List windows — the modal MessageBox should be a new top-level window
        // owned by the same pid.
        let resp = driver.call("list_windows", serde_json::json!({
            "pid": pid as i64
        }));
        let windows = resp.structured()["windows"].as_array()
            .expect("windows array");
        let modal = windows.iter()
            .find(|w| w["title"].as_str().map(|t| t.contains("Harness MessageBox")).unwrap_or(false))
            .expect("Harness MessageBox modal window not found");
        let modal_wid = modal["window_id"].as_u64().unwrap();
        println!("modal window_id={}", modal_wid);

        // Walk the modal's UIA tree — expect OK and Cancel buttons.
        let modal_snap = driver.call("get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": modal_wid, "capture_mode": "tree"
        }));
        let modal_text = modal_snap.text();
        assert!(modal_text.contains("\"OK\""),
            "MessageBox UIA tree missing OK button. Tree: {}",
            modal_text.chars().take(800).collect::<String>());
        assert!(modal_text.contains("\"Cancel\""),
            "MessageBox UIA tree missing Cancel button");

        // Dismiss by clicking Cancel in the modal.
        let cancel_idx = modal_text.lines()
            .find(|l| l.contains("\"Cancel\"") && l.contains('['))
            .and_then(|l| {
                let s = l.find('[')? + 1;
                let e = l[s..].find(']')? + s;
                l[s..e].trim().parse::<u64>().ok()
            })
            .expect("Cancel button element_index not parseable");
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": modal_wid, "element_index": cancel_idx
        }));
        std::thread::sleep(Duration::from_millis(400));
        println!("✅ harness_wpf_modal_messagebox: opened + parsed + dismissed");
    });
}

#[test]
#[ignore]
fn harness_wpf_owned_popup() {
    with_session(|pid, wid, driver| {
        let snap = snapshot(driver, pid, wid);
        let idx = ax::element_index_by_id(snap.text(), "btn-open-owned")
            .expect("btn-open-owned not in snapshot");
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx
        }));
        std::thread::sleep(Duration::from_millis(500));

        let resp = driver.call("list_windows", serde_json::json!({
            "pid": pid as i64
        }));
        let windows = resp.structured()["windows"].as_array().unwrap();
        let owned = windows.iter()
            .find(|w| w["title"].as_str().map(|t| t.contains("Harness Owned Popup")).unwrap_or(false))
            .expect("Harness Owned Popup window not found in list_windows");
        let owned_wid = owned["window_id"].as_u64().unwrap();

        let owned_snap = driver.call("get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": owned_wid, "capture_mode": "tree"
        }));
        let owned_text = owned_snap.text();
        assert!(owned_text.contains("OWNED_POPUP_MARKER_v1"),
            "owned popup body marker missing. Tree: {}",
            owned_text.chars().take(600).collect::<String>());
        println!("✅ harness_wpf_owned_popup: opened + parsed");
    });
}

#[test]
#[ignore]
fn harness_wpf_layered_popup_capture() {
    with_session(|pid, wid, driver| {
        let snap = snapshot(driver, pid, wid);
        let idx = ax::element_index_by_id(snap.text(), "btn-open-layered")
            .expect("btn-open-layered not in snapshot");
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx
        }));
        std::thread::sleep(Duration::from_millis(600));

        let resp = driver.call("list_windows", serde_json::json!({
            "pid": pid as i64
        }));
        let windows = resp.structured()["windows"].as_array().unwrap();
        let layered = windows.iter()
            .find(|w| w["title"].as_str().map(|t| t.contains("Harness Layered Popup")).unwrap_or(false))
            .expect("Harness Layered Popup window not found");
        let layered_wid = layered["window_id"].as_u64().unwrap();

        // Capture-only path — assert the screenshot is not all-black, which
        // is the failure mode for PrintWindow against layered windows
        // without the WGC fallback.
        let cap = driver.call("get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": layered_wid, "capture_mode": "vision"
        }));
        let img_b64 = cap.raw["result"]["content"].as_array()
            .and_then(|arr| arr.iter().find_map(|c| {
                if c["type"] == "image" { c["data"].as_str() } else { None }
            }))
            .expect("layered window capture returned no image");
        // Decode the PNG and look for any non-black pixel.
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, img_b64)
            .expect("base64");
        let img = image::load_from_memory(&bytes).expect("png decode");
        let rgb = img.to_rgb8();
        let any_color = rgb.pixels().any(|p| p.0[0] > 12 || p.0[1] > 12 || p.0[2] > 12);
        assert!(any_color,
            "layered window capture is all-black ({}x{}). PrintWindow likely needs WGC fallback.",
            rgb.width(), rgb.height());
        println!("✅ harness_wpf_layered_popup_capture: capture has non-black pixels ({}x{})",
                 rgb.width(), rgb.height());
    });
}

// ── slider / checkable / combo / list / menu coverage ────────────────────────

#[test]
#[ignore]
fn harness_wpf_slider_drag() {
    // Regression guard for the SendInput drag path. PostMessage drag
    // doesn't update GetKeyState, so WPF's Thumb-drag handler (which
    // polls Mouse.LeftButton via GetKeyState) never sees the button
    // held — the thumb stays put. dispatch:"foreground" routes through
    // send_drag_synthesized which goes via the system input queue and
    // DOES update GetKeyState, so the thumb actually tracks.
    //
    // Foreground-lock caveat: SetForegroundWindow can be rejected from
    // non-UIAccess processes during the daemon-process foreground swap.
    // bring_to_front first to make the harness foreground (via
    // AttachThreadInput), then SendInput's own SetForegroundWindow is a
    // no-op success.
    with_session(|pid, wid, driver| {
        focus_harness(driver, pid, wid);
        let pre = snapshot(driver, pid, wid);
        assert!(pre.text().contains("slider_value=0"),
            "initial slider_value=0 missing");

        let resp = driver.call("drag", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            // Window-local coords along the slider TRACK. The track row sits at
            // window-local y≈304 (verified on the VM: y=275 landed ~29px above
            // it, on empty GroupBox space, so the thumb never moved); the thumb
            // rests at the left (x≈44) at value=0. Dragging left→right advances
            // the value. (TODO: derive these from the `sld-value` element frame
            // in get_window_state for DPI/placement independence.)
            "from_x": 44.0, "from_y": 304.0,
            "to_x": 330.0, "to_y": 304.0,
            "duration_ms": 700, "steps": 40,
            "dispatch": "foreground"
        }));
        let msg = resp.text();
        println!("drag slider (foreground): {msg}");
        assert!(msg.starts_with("✅"),
            "drag tool returned non-success: {msg}");
        std::thread::sleep(Duration::from_millis(500));

        let post = snapshot(driver, pid, wid);
        let text = post.text();
        let advanced = text.lines().any(|l|
            l.contains("slider_value=") && !l.contains("slider_value=0\""));
        assert!(advanced,
            "Slider value did not advance via SendInput drag. Lines: {}",
            text.lines().filter(|l| l.contains("slider_value"))
                .collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_slider_drag: thumb tracked via SendInput drag");
    });
}

#[test]
#[ignore]
fn harness_wpf_slider_increase_large() {
    // Companion to slider_drag — exercises UIA Invoke on the Slider's
    // internal IncreaseLarge "page-up" button. Doesn't depend on screen
    // coords, so it's the more robust slider integration test.
    with_session(|pid, wid, driver| {
        focus_harness(driver, pid, wid);
        let snap = snapshot(driver, pid, wid);
        let idx = ax::element_index_by_id(snap.text(), "IncreaseLarge")
            .expect("slider IncreaseLarge button not in snapshot");
        for i in 0..3 {
            let resp = driver.call("click", serde_json::json!({
                "pid": pid as i64, "window_id": wid, "element_index": idx
            }));
            println!("invoke IncreaseLarge #{i}: {}", resp.text());
            std::thread::sleep(Duration::from_millis(150));
        }
        std::thread::sleep(Duration::from_millis(300));
        let post = snapshot(driver, pid, wid);
        let text = post.text();
        let advanced = text.lines().any(|l|
            l.contains("slider_value=") && !l.contains("slider_value=0\""));
        assert!(advanced, "slider IncreaseLarge invokes did not advance value. Lines: {}",
            text.lines().filter(|l| l.contains("slider_value")).collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_slider_increase_large: advanced via UIA Invoke");
    });
}

#[test]
#[ignore]
fn harness_wpf_checkbox_toggle() {
    with_session(|pid, wid, driver| {
        focus_harness(driver, pid, wid);
        let snap = snapshot(driver, pid, wid);
        let idx = ax::element_index_by_id(snap.text(), "chk-agreed")
            .expect("chk-agreed missing");
        // CheckBox exposes UIA TogglePattern (actions=[toggle]), not Invoke.
        // cua-driver's click tool tries UIA Invoke first; for elements that
        // don't support it the PostMessage fallback path runs. Use
        // dispatch:"foreground" to land a SendInput click that WPF
        // recognises as a real user click and processes through Toggle.
        let resp = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "dispatch": "foreground"
        }));
        println!("click chk-agreed: {}", resp.text());
        std::thread::sleep(Duration::from_millis(400));
        let post = snapshot(driver, pid, wid);
        assert!(post.text().contains("agreed=True"),
            "checkbox didn't toggle: {}",
            post.text().lines().filter(|l| l.contains("agreed=")).collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_checkbox_toggle: agreed=True");
    });
}

#[test]
#[ignore]
fn harness_wpf_radio_select() {
    with_session(|pid, wid, driver| {
        focus_harness(driver, pid, wid);
        let snap = snapshot(driver, pid, wid);
        let idx = ax::element_index_by_id(snap.text(), "rdo-high")
            .expect("rdo-high missing");
        // RadioButton exposes SelectionItem pattern (actions=[select]).
        // Same dispatch:foreground rationale as the checkbox test.
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(400));
        let post = snapshot(driver, pid, wid);
        assert!(post.text().contains("prio=High"),
            "radio didn't switch to High");
        println!("✅ harness_wpf_radio_select: prio=High");
    });
}

#[test]
#[ignore]
fn harness_wpf_combo_select() {
    with_session(|pid, wid, driver| {
        focus_harness(driver, pid, wid);
        let snap = snapshot(driver, pid, wid);
        let combo_idx = ax::element_index_by_id(snap.text(), "cbo-color")
            .expect("cbo-color missing");
        // WPF ComboBox UIA peer surfaces ExpandCollapsePattern (actions=[expand])
        // but not ValuePattern — set_value at the parent is a no-op. Standard
        // recipe: invoke the combo to expand the dropdown, re-snapshot so the
        // item AIDs land in the element cache, then click the target item.
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": combo_idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(500));

        let snap2 = snapshot(driver, pid, wid);
        let item_idx = ax::element_index_by_id(snap2.text(), "cbo-item-orange")
            .expect("cbo-item-orange missing after expand");
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": item_idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(500));

        let post = snapshot(driver, pid, wid);
        assert!(post.text().contains("color=orange"),
            "combo didn't switch to orange: {}",
            post.text().lines().filter(|l| l.contains("color=")).collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_combo_select: color=orange");
    });
}

#[test]
#[ignore]
fn harness_wpf_listbox_select() {
    with_session(|pid, wid, driver| {
        focus_harness(driver, pid, wid);
        let snap = snapshot(driver, pid, wid);
        let idx = ax::element_index_by_id(snap.text(), "lst-cherry")
            .expect("lst-cherry missing");
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(400));
        let post = snapshot(driver, pid, wid);
        assert!(post.text().contains("selected=cherry"),
            "list didn't select cherry: {}",
            post.text().lines().filter(|l| l.contains("selected=")).collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_listbox_select: selected=cherry");
    });
}

#[test]
#[ignore]
fn harness_wpf_menu_invoke() {
    with_session(|pid, wid, driver| {
        focus_harness(driver, pid, wid);
        // Expand File menu first (UIA expand pattern on MenuItem)
        let snap = snapshot(driver, pid, wid);
        let file_idx = ax::element_index_by_id(snap.text(), "menu-file")
            .expect("menu-file missing");
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": file_idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(400));

        // Re-snapshot so menu-file-new is in the cache (it materialized
        // when the menu expanded).
        let snap2 = snapshot(driver, pid, wid);
        let new_idx = ax::element_index_by_id(snap2.text(), "menu-file-new")
            .expect("menu-file-new missing after expand");
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": new_idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(500));

        let post = snapshot(driver, pid, wid);
        assert!(post.text().contains("menu_action=file_new"),
            "File>New didn't invoke: {}",
            post.text().lines().filter(|l| l.contains("menu_action=")).collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_menu_invoke: menu_action=file_new");
    });
}
