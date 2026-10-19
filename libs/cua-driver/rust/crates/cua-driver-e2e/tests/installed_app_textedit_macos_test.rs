//! TextEdit delivery integration checks for macOS.
//!
//! Covers the accessibility route and confirmed value read-back on a real
//! Cocoa app, then reads the editor back through a fresh snapshot, and the
//! window-only foreground chord while another app holds the front or while a
//! sibling document of the same app holds the keyboard focus.
//!
//! The schema contract lives in `protocol_schema_test.rs`. These installed-app
//! checks run in the canonical logged-in macOS desktop lane.

#![cfg(target_os = "macos")]

use std::process::Command;
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Observation, OracleKind,
    Scope, Targeting,
};
use cua_driver_testkit::{Driver, McpDriver};

const FINDER_BUNDLE: &str = "com.apple.finder";
const TEXTEDIT_BUNDLE: &str = "com.apple.TextEdit";

struct TextEditDocument {
    pid: i64,
    window_id: u64,
    element_token: String,
}

/// TextEdit may restore an earlier unsaved document, so read-back oracles need
/// text that no earlier run could have left behind.
fn unique_marker(prefix: &str) -> String {
    format!(
        "{prefix}{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after the Unix epoch")
            .as_nanos()
    )
}

/// Launch TextEdit and locate the on-screen window that exposes the editor.
///
/// TextEdit can retain an off-screen restoration window alongside the visible
/// blank document, and the document window can appear a moment after launch.
/// WindowServer does not guarantee their order, so prefer visible windows and
/// select the one that actually exposes the editor instead of assuming
/// `windows[0]` is the document.
fn launch_textedit_document(driver: &mut McpDriver) -> TextEditDocument {
    let launch = driver.call(
        "launch_app",
        serde_json::json!({ "bundle_id": TEXTEDIT_BUNDLE }),
    );
    assert!(
        !launch.is_error(),
        "could not launch TextEdit: {}",
        launch.text()
    );
    let pid = launch.structured()["pid"].as_i64().expect("TextEdit pid");
    let mut windows = launch.structured()["windows"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        windows.sort_by_key(|window| !window["is_on_screen"].as_bool().unwrap_or(false));
        let editor = windows
            .iter()
            .filter_map(|window| window["window_id"].as_u64())
            .find_map(|window_id| {
                let state = driver.call(
                    "get_window_state",
                    serde_json::json!({
                        "pid": pid,
                        "window_id": window_id,
                        "capture_mode": "ax"
                    }),
                );
                state.structured()["elements"]
                    .as_array()
                    .and_then(|elements| {
                        elements
                            .iter()
                            .find(|element| element["role"] == "AXTextArea")
                            .and_then(|element| element["element_token"].as_str())
                    })
                    .map(|element_token| (window_id, element_token.to_owned()))
            });
        if let Some((window_id, element_token)) = editor {
            return TextEditDocument {
                pid,
                window_id,
                element_token,
            };
        }
        assert!(
            Instant::now() < deadline,
            "TextEdit opened no window containing an AXTextArea: {windows:?}"
        );
        std::thread::sleep(Duration::from_millis(250));
        let listed = driver.call("list_windows", serde_json::json!({}));
        windows = listed.structured()["windows"]
            .as_array()
            .map(|listed| {
                listed
                    .iter()
                    .filter(|window| window["pid"].as_i64() == Some(pid))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
    }
}

/// The driver's own read-back is not the oracle. Take fresh snapshots until the
/// editor holds the expected text, and return the editor's token from that
/// snapshot since each snapshot retires the earlier tokens.
fn wait_for_editor_text(
    driver: &mut McpDriver,
    document: &TextEditDocument,
    accept: impl Fn(&str) -> bool,
    expectation: &str,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = driver.call(
            "get_window_state",
            serde_json::json!({
                "pid": document.pid,
                "window_id": document.window_id,
                "capture_mode": "ax"
            }),
        );
        let landed = state.structured()["elements"]
            .as_array()
            .and_then(|elements| {
                elements.iter().find(|element| {
                    element["role"] == "AXTextArea"
                        && element["value"].as_str().is_some_and(&accept)
                })
            })
            .and_then(|element| element["element_token"].as_str());
        if let Some(element_token) = landed {
            return element_token.to_owned();
        }
        assert!(
            Instant::now() < deadline,
            "TextEdit's AXTextArea never {expectation}:\n{}",
            state.text()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn osascript(script: &str) -> String {
    let output = Command::new("osascript")
        .args(["-e", script])
        .output()
        .expect("run osascript");
    assert!(
        output.status.success(),
        "osascript failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn frontmost_bundle_id() -> String {
    osascript(
        r#"tell application "System Events" to bundle identifier of first application process whose frontmost is true"#,
    )
}

fn wait_for_frontmost(bundle_id: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if frontmost_bundle_id() == bundle_id {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn ensure_frontmost(bundle_id: &str) {
    for _ in 0..3 {
        osascript(&format!(r#"tell application id "{bundle_id}" to activate"#));
        if wait_for_frontmost(bundle_id) {
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!("could not make {bundle_id} frontmost for the TextEdit foreground check");
}

fn focused_textedit_window_title() -> String {
    osascript(
        r#"tell application "System Events" to tell process "TextEdit" to get name of value of attribute "AXFocusedWindow""#,
    )
}

/// Open a second TextEdit document. TextEdit focuses the new document, so the
/// first one becomes an unfocused sibling of a frontmost app.
fn open_sibling_document(text: &str) {
    osascript(&format!(
        r#"tell application id "{TEXTEDIT_BUNDLE}" to make new document with properties {{text:"{text}"}}"#
    ));
    ensure_frontmost(TEXTEDIT_BUNDLE);
    let deadline = Instant::now() + Duration::from_secs(5);
    while focused_textedit_window_title() != "Untitled 2" {
        assert!(
            Instant::now() < deadline,
            "the sibling document never took the keyboard focus; focused window is {:?}",
            focused_textedit_window_title()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ── End-to-end ladder behavior (interactive; needs a GUI session) ────────────

/// On a NATIVE Cocoa field (TextEdit), `delivery_mode:"background"` lands via the
/// AX value-write and the driver confirms it with accessibility read-back. This is
/// the driver-verifiable happy path — no foreground needed, no screenshot needed.
#[test]
#[ignore]
fn background_type_on_native_cocoa_is_ax_verified() {
    use cua_driver_testkit::observer::TargetWindow;
    use cua_driver_testkit::sentinel::run_with_background_oracles;

    let cell_id = "macos-textedit-type-text-ax-background";
    let marker = unique_marker("ladder");
    let case = CaseSpec::delivered(
        cell_id,
        "textedit",
        "appkit",
        "type_text",
        Targeting::Ax,
        Delivery::Background,
        Scope::Window,
        DriverRoute::MacosAxValue,
        vec![
            OracleKind::AxState,
            OracleKind::Focus,
            OracleKind::ZOrder,
            OracleKind::Cursor,
            OracleKind::NoLeakedInput,
        ],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named(cell_id)
            .expect("start installed macOS daemon proxy");
        *evidence = recording_evidence(driver.recording_dir());
        let document = launch_textedit_document(&mut driver);

        let (typed, mut passed) = run_with_background_oracles(
            &mut driver,
            TargetWindow {
                pid: document.pid as u32,
                native_id: document.window_id,
            },
            |driver| {
                driver.call(
                    "type_text",
                    serde_json::json!({
                        "pid": document.pid, "window_id": document.window_id,
                        "element_token": document.element_token,
                        "text": marker, "delivery_mode": "background"
                    }),
                )
            },
        )
        .unwrap_or_else(|error| panic!("background TextEdit contract failed: {error}"));
        assert!(!typed.is_error(), "type_text errored: {}", typed.text());
        assert_eq!(
            typed.action_route(),
            Some("accessibility"),
            "native Cocoa field should land via AX: {}",
            typed.text()
        );
        assert_eq!(
            typed.action_effect(),
            Some("confirmed"),
            "AX write should be confirmed by read-back: {}",
            typed.text()
        );
        assert_eq!(
            typed.structured()["evidence"][0]["kind"],
            "value_readback",
            "confirmed actions must expose publishable evidence: {}",
            typed.text()
        );
        wait_for_editor_text(
            &mut driver,
            &document,
            |value| value.contains(&marker),
            &format!("held {marker:?} after a confirmed write"),
        );
        passed.push(OracleKind::AxState);
        Observation::delivered(passed, Default::default())
    });
}

/// A window-only `delivery_mode:"foreground"` chord reaches the TextEdit
/// document while Finder holds the front, and Finder gets the front back. The
/// selection is proven by the next AX insert replacing the whole document
/// rather than appending to it.
#[test]
#[ignore]
fn foreground_window_hotkey_selects_all_behind_another_app() {
    let cell_id = "macos-textedit-hotkey-window-foreground";
    let seed = unique_marker("seed");
    let replacement = unique_marker("replaced");
    let case = CaseSpec::delivered(
        cell_id,
        "textedit",
        "appkit",
        "hotkey",
        Targeting::NotApplicable,
        Delivery::Foreground,
        Scope::Window,
        DriverRoute::MacosCgEventHid,
        vec![OracleKind::AxState, OracleKind::Focus],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named(cell_id)
            .expect("start installed macOS daemon proxy");
        *evidence = recording_evidence(driver.recording_dir());
        let document = launch_textedit_document(&mut driver);

        let seeded = driver.call(
            "type_text",
            serde_json::json!({
                "pid": document.pid, "window_id": document.window_id,
                "element_token": document.element_token,
                "text": seed, "delivery_mode": "background"
            }),
        );
        assert!(!seeded.is_error(), "seeding failed: {}", seeded.text());
        let element_token = wait_for_editor_text(
            &mut driver,
            &document,
            |value| value.contains(&seed),
            &format!("held the seed {seed:?}"),
        );

        ensure_frontmost(FINDER_BUNDLE);
        driver.start_behavior_recording();
        let hotkey = driver.call(
            "hotkey",
            serde_json::json!({
                "pid": document.pid, "window_id": document.window_id,
                "keys": ["cmd", "a"], "delivery_mode": "foreground"
            }),
        );
        assert!(
            !hotkey.is_error(),
            "foreground hotkey failed: {}",
            hotkey.text()
        );
        assert!(
            wait_for_frontmost(FINDER_BUNDLE),
            "Finder did not get the front back after the hotkey; frontmost is {}",
            frontmost_bundle_id()
        );

        let typed = driver.call(
            "type_text",
            serde_json::json!({
                "pid": document.pid, "window_id": document.window_id,
                "element_token": element_token,
                "text": replacement, "delivery_mode": "background"
            }),
        );
        assert!(!typed.is_error(), "type_text errored: {}", typed.text());
        wait_for_editor_text(
            &mut driver,
            &document,
            |value| value.contains(&replacement) && !value.contains(&seed),
            &format!("replaced {seed:?} with {replacement:?}: cmd+a did not select the document"),
        );
        Observation::delivered(
            vec![OracleKind::AxState, OracleKind::Focus],
            Default::default(),
        )
    });
}

/// A window-only `delivery_mode:"foreground"` chord addressed at a document
/// of the frontmost app reaches that document even while a sibling document
/// holds the keyboard focus.
#[test]
#[ignore]
fn foreground_window_hotkey_reaches_an_unfocused_sibling_of_the_frontmost_app() {
    let cell_id = "macos-textedit-hotkey-window-foreground-sibling";
    let seed = unique_marker("seed");
    let replacement = unique_marker("replaced");
    let case = CaseSpec::delivered(
        cell_id,
        "textedit",
        "appkit",
        "hotkey",
        Targeting::NotApplicable,
        Delivery::Foreground,
        Scope::Window,
        DriverRoute::MacosCgEventHid,
        vec![OracleKind::AxState, OracleKind::Focus],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named(cell_id)
            .expect("start installed macOS daemon proxy");
        *evidence = recording_evidence(driver.recording_dir());
        let document = launch_textedit_document(&mut driver);

        let seeded = driver.call(
            "type_text",
            serde_json::json!({
                "pid": document.pid, "window_id": document.window_id,
                "element_token": document.element_token,
                "text": seed, "delivery_mode": "background"
            }),
        );
        assert!(!seeded.is_error(), "seeding failed: {}", seeded.text());
        let element_token = wait_for_editor_text(
            &mut driver,
            &document,
            |value| value.contains(&seed),
            &format!("held the seed {seed:?}"),
        );

        open_sibling_document(&unique_marker("sibling"));
        driver.start_behavior_recording();
        let hotkey = driver.call(
            "hotkey",
            serde_json::json!({
                "pid": document.pid, "window_id": document.window_id,
                "keys": ["cmd", "a"], "delivery_mode": "foreground"
            }),
        );
        assert!(
            !hotkey.is_error(),
            "foreground hotkey failed: {}",
            hotkey.text()
        );
        assert!(
            wait_for_frontmost(TEXTEDIT_BUNDLE),
            "TextEdit lost the front after a chord addressed at its own window; frontmost is {}",
            frontmost_bundle_id()
        );

        let typed = driver.call(
            "type_text",
            serde_json::json!({
                "pid": document.pid, "window_id": document.window_id,
                "element_token": element_token,
                "text": replacement, "delivery_mode": "background"
            }),
        );
        assert!(!typed.is_error(), "type_text errored: {}", typed.text());
        wait_for_editor_text(
            &mut driver,
            &document,
            |value| value.contains(&replacement) && !value.contains(&seed),
            &format!("replaced {seed:?} with {replacement:?}: cmd+a did not select the document"),
        );
        Observation::delivered(
            vec![OracleKind::AxState, OracleKind::Focus],
            Default::default(),
        )
    });
}
