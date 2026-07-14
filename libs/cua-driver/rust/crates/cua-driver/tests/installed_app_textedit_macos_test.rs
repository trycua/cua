//! TextEdit background-delivery integration check for macOS.
//!
//! Covers the `{path, verified}` structured outcome on a real Cocoa app.
//!
//! The schema contract lives in `protocol_schema_test.rs`. This installed-app
//! check runs in the canonical logged-in macOS desktop lane.

#![cfg(target_os = "macos")]

// ── End-to-end ladder behavior (interactive; needs a GUI session) ────────────

/// On a NATIVE Cocoa field (TextEdit), `delivery_mode:"background"` lands via the
/// AX value-write and the driver confirms it: `path:"ax", verified:true`. This is
/// the driver-verifiable happy path — no foreground needed, no screenshot needed.
#[test]
#[ignore]
fn background_type_on_native_cocoa_is_ax_verified() {
    use cua_driver_testkit::e2e::{
        execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Observation, OracleKind,
        Scope, Targeting,
    };
    use cua_driver_testkit::observer::TargetWindow;
    use cua_driver_testkit::sentinel::run_with_background_oracles;
    use cua_driver_testkit::{Driver, McpDriver};

    let cell_id = "macos-textedit-type-text-ax-background";
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

        // Launch TextEdit and open a blank document.
        let launch = driver.call(
            "launch_app",
            serde_json::json!({ "bundle_id": "com.apple.TextEdit" }),
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
        assert!(!windows.is_empty(), "TextEdit opened no window");

        // TextEdit can retain an off-screen restoration window alongside the
        // visible blank document. WindowServer does not guarantee their order,
        // so prefer visible windows and select the one that actually exposes
        // the editor instead of assuming `windows[0]` is the document.
        windows.sort_by_key(|window| !window["is_on_screen"].as_bool().unwrap_or(false));
        let (wid, el) = windows
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
                            .and_then(|element| element["element_index"].as_u64())
                    })
                    .map(|element_index| (window_id, element_index))
            })
            .expect("TextEdit opened no window containing an AXTextArea");

        let (typed, mut passed) = run_with_background_oracles(
            &mut driver,
            TargetWindow {
                pid: pid as u32,
                native_id: wid,
            },
            |driver| {
                driver.call(
                    "type_text",
                    serde_json::json!({
                        "pid": pid, "window_id": wid, "element_index": el,
                        "text": "ladder", "delivery_mode": "background"
                    }),
                )
            },
        )
        .unwrap_or_else(|error| panic!("background TextEdit contract failed: {error}"));
        assert!(!typed.is_error(), "type_text errored: {}", typed.text());
        assert_eq!(
            typed.path(),
            Some("ax"),
            "native Cocoa field should land via AX: {}",
            typed.text()
        );
        assert_eq!(
            typed.verified(),
            Some(true),
            "AX write should read back as verified: {}",
            typed.text()
        );
        passed.push(OracleKind::AxState);
        Observation::delivered(passed, Default::default())
    });
}
