//! Protocol integration tests for snapshot-bound element targeting.
//!
//! These exercise the MCP protocol surface (schema + capability claims)
//! by spawning the real `cua-driver` binary and walking `tools/list`.
//! Unit tests for the resolution logic live in
//! `cua-driver-core::element_token` and the per-platform tool modules —
//! this file is only for what's visible across the JSON-RPC boundary.

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

/// Every element-addressed tool advertises both safe target forms: an opaque
/// token, or an integer paired with its snapshot id.
#[test]
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

        // (a) inputSchema exposes the three related target fields.
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
        assert_eq!(
            props["element_index"]["type"].as_str(),
            Some("integer"),
            "{tool_name} element_index must remain available only as a snapshot-bound pair"
        );
        assert_eq!(
            props["snapshot_id"]["type"].as_str(),
            Some("string"),
            "{tool_name} must advertise snapshot_id for safe integer targeting"
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

/// `get_window_state` claims `accessibility.element_tokens`
/// because it EMITS the tokens (the other side of the contract from
/// the action tools above).
#[test]
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

/// The native menu operation is a path contract, never another entry point for
/// mutable snapshot indices.
#[test]
fn invoke_menu_is_path_only_and_claims_native_menu_capability() {
    let Some(resp) = fetch_tools_list() else {
        return;
    };
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    let tool = tools
        .iter()
        .find(|tool| tool["name"].as_str() == Some("invoke_menu"))
        .expect("invoke_menu missing");
    let properties = tool["inputSchema"]["properties"]
        .as_object()
        .expect("invoke_menu properties");
    assert!(properties.contains_key("path"));
    assert!(!properties.contains_key("element_index"));
    assert!(!properties.contains_key("element_token"));
    let capabilities = tool["capabilities"].as_array().expect("capabilities");
    assert!(capabilities.iter().any(|value| value == "menu.path.invoke"));
    assert!(capabilities
        .iter()
        .any(|value| value == "accessibility.menu.native"));
}
