//! Modality axis: the best-effort-background dispatch ladder (macOS).
//!
//! Covers the contract introduced by the modality-ladder work:
//!   - `delivery_mode` (background | foreground) on the input tools,
//!   - `scope` (window | desktop) as a per-call param on `click`,
//!   - the `{path, verified}` structured outcome,
//!
//! The schema assertions are deterministic and run in CI (they only inspect
//! `tools/list`). The end-to-end ladder behavior is `#[ignore]` — it needs a
//! real focus-sensitive app + a GUI session, so it runs interactively / on the
//! VMs, asserting per-rung outcomes via the testkit `path()` / `verified()`
//! accessors instead of scraping result text.

#![cfg(target_os = "macos")]

use cua_driver_testkit::RawDriver;

/// `delivery_mode` (type_text + click) and `scope` (click) are advertised with
/// their enums in `tools/list`. Consumers branch on these being present.
#[test]
fn dispatch_and_scope_schema_advertised() {
    let Some(mut d) = RawDriver::spawn() else { return };
    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();
    d.send(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let resp = d.recv();
    let tools = resp["result"]["tools"].as_array().expect("tools array");

    let props = |name: &str| {
        tools.iter().find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} not in tools/list"))
            ["inputSchema"]["properties"].clone()
    };

    let enum_values = |v: &serde_json::Value| -> Vec<String> {
        v["enum"].as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    };

    for tool in ["type_text", "click"] {
        let dm = props(tool)["delivery_mode"].clone();
        let en = enum_values(&dm);
        assert!(
            en.iter().any(|s| s == "background") && en.iter().any(|s| s == "foreground"),
            "{tool}.delivery_mode enum should be [background, foreground], got {dm:?}"
        );
    }

    let scope = props("click")["scope"].clone();
    let sen = enum_values(&scope);
    assert!(
        sen.iter().any(|s| s == "window") && sen.iter().any(|s| s == "desktop"),
        "click.scope enum should be [window, desktop], got {scope:?}"
    );
}

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
