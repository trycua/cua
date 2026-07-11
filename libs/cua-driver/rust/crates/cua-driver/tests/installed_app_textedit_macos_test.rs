//! Optional TextEdit background-delivery integration check for macOS.
//!
//! Covers the `{path, verified}` structured outcome on a real Cocoa app.
//!
//! The schema contract lives in `protocol_schema_test.rs`. This optional
//! installed-app check needs a focus-sensitive app and GUI session.

#![cfg(target_os = "macos")]

// ── End-to-end ladder behavior (interactive; needs a GUI session) ────────────

/// On a NATIVE Cocoa field (TextEdit), `delivery_mode:"background"` lands via the
/// AX value-write and the driver confirms it: `path:"ax", verified:true`. This is
/// the driver-verifiable happy path — no foreground needed, no screenshot needed.
#[test]
#[ignore]
fn background_type_on_native_cocoa_is_ax_verified() {
    use cua_driver_testkit::{Driver, McpDriver};
    let Some(mut driver) = McpDriver::spawn() else { return };

    // Launch TextEdit and open a blank document.
    let launch = driver.call("launch_app", serde_json::json!({ "bundle_id": "com.apple.TextEdit" }));
    if launch.is_error() {
        eprintln!("[dispatch] could not launch TextEdit — skipping");
        return;
    }
    let pid = launch.structured()["pid"].as_i64().expect("pid");
    let windows = launch.structured()["windows"].as_array().cloned().unwrap_or_default();
    let Some(wid) = windows.first().and_then(|w| w["window_id"].as_u64()) else {
        eprintln!("[dispatch] TextEdit opened no window — skipping");
        return;
    };

    // Find the AXTextArea.
    let state = driver.call("get_window_state",
        serde_json::json!({ "pid": pid, "window_id": wid, "capture_mode": "ax" }));
    let el = state.structured()["elements"].as_array().and_then(|els| {
        els.iter().find(|e| e["role"] == "AXTextArea").and_then(|e| e["element_index"].as_u64())
    });
    let Some(el) = el else {
        eprintln!("[dispatch] no AXTextArea in TextEdit (AX permission?) — skipping");
        return;
    };

    let typed = driver.call("type_text", serde_json::json!({
        "pid": pid, "window_id": wid, "element_index": el,
        "text": "ladder", "delivery_mode": "background"
    }));
    assert!(!typed.is_error(), "type_text errored: {}", typed.text());
    assert_eq!(typed.path(), Some("ax"), "native Cocoa field should land via AX: {}", typed.text());
    assert_eq!(typed.verified(), Some(true), "AX write should read back as verified: {}", typed.text());
}
