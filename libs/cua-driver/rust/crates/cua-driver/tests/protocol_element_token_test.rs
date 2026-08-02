//! Integration tests for Surface 6 — opaque `element_token` alongside
//! the existing integer `element_index`.
//!
//! These exercise the MCP protocol surface (schema + capability claims)
//! by spawning the real `cua-driver` binary and walking `tools/list`.
//! Unit tests for the resolution logic live in
//! `cua-driver-core::element_token` and the per-platform tool modules —
//! this file is only for what's visible across the JSON-RPC boundary.

// element_token resolution is exercised on macOS/Linux only; gate the whole
// file (helpers included) so Windows excludes it instead of warning about the
// then-unused tools/list helpers.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use cua_driver_testkit::RawDriver;

/// Spawn the driver, send initialize + tools/list, return the parsed
/// `tools/list` response. Skips the test silently if the binary hasn't
/// been built (CI builds it separately).
fn fetch_tools_list() -> Option<serde_json::Value> {
    let mut driver = RawDriver::spawn()?;
    driver.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
    }));
    driver.recv();
    driver.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/list"
    }));
    Some(driver.recv())
}

/// Surface 6: Every tool that accepts the opaque `element_token` arg
/// must (a) advertise the field in its inputSchema and (b) claim the
/// `accessibility.element_tokens` capability. These together let
/// consumers detect support before issuing a token-carrying call.
#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn token_accepting_tools_advertise_element_token_in_schema_and_capabilities() {
    let resp = match fetch_tools_list() {
        Some(r) => r,
        None => return,
    };
    let tools = resp["result"]["tools"]
        .as_array()
        .expect("tools/list response missing tools array");

    // Tools that gained `element_token` in Surface 6. Keep in sync with
    // the per-platform schema additions + the centralised
    // `default_capabilities_for` map in cua-driver-core.
    const TOKEN_TOOLS: &[&str] = &[
        "click",
        "double_click",
        "right_click",
        "scroll",
        "type_text",
        "press_key",
        "set_value",
    ];

    for tool_name in TOKEN_TOOLS {
        let tool = tools
            .iter()
            .find(|t| t["name"].as_str() == Some(tool_name))
            .unwrap_or_else(|| panic!("tool {tool_name:?} missing from tools/list"));

        // (a) inputSchema.properties.element_token present, type=string.
        let props = tool["inputSchema"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{tool_name} has no inputSchema.properties"));
        let et = props.get("element_token").unwrap_or_else(|| {
            panic!(
                "Surface 6: {tool_name} must advertise an `element_token` input field — \
                 missing from schema properties: {props:?}"
            )
        });
        assert_eq!(
            et["type"].as_str(),
            Some("string"),
            "{tool_name} element_token must be type:string"
        );

        // (b) capabilities array includes `accessibility.element_tokens`.
        let caps: Vec<&str> = tool["capabilities"]
            .as_array()
            .unwrap_or_else(|| panic!("{tool_name} capabilities missing"))
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(
            caps.contains(&"accessibility.element_tokens"),
            "Surface 6: {tool_name} must claim `accessibility.element_tokens` — \
             current claim is {caps:?}"
        );
    }
}

/// Surface 6: `get_window_state` claims `accessibility.element_tokens`
/// because it EMITS the tokens (the other side of the contract from
/// the action tools above).
#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn get_window_state_claims_element_tokens_capability() {
    let resp = match fetch_tools_list() {
        Some(r) => r,
        None => return,
    };
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    let gws = tools
        .iter()
        .find(|t| t["name"].as_str() == Some("get_window_state"))
        .expect("get_window_state missing");
    let caps: Vec<&str> = gws["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        caps.contains(&"accessibility.element_tokens"),
        "Surface 6: get_window_state must claim accessibility.element_tokens \
         (it emits the tokens). Current claims: {caps:?}"
    );
}

/// Surface 6 hard constraint: `element_index` MUST still be present on
/// every token-accepting tool's schema — the new `element_token` field
/// is additive, not a replacement. This is the regression guard for
/// the back-compat contract.
#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn element_index_field_still_present_on_token_accepting_tools() {
    let resp = match fetch_tools_list() {
        Some(r) => r,
        None => return,
    };
    let tools = resp["result"]["tools"].as_array().expect("tools array");

    const TOKEN_TOOLS: &[&str] = &[
        "click",
        "double_click",
        "right_click",
        "scroll",
        "type_text",
        "press_key",
        "set_value",
    ];

    for tool_name in TOKEN_TOOLS {
        let tool = tools
            .iter()
            .find(|t| t["name"].as_str() == Some(tool_name))
            .unwrap_or_else(|| panic!("{tool_name} missing"));
        let props = tool["inputSchema"]["properties"].as_object().unwrap();
        assert!(
            props.contains_key("element_index"),
            "Surface 6 hard constraint: {tool_name}.element_index must \
             stay on the schema — element_token is purely additive"
        );
    }
}
