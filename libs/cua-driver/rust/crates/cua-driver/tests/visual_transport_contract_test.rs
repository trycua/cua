//! Callable visual perception must advertise and preserve one contract across
//! the live MCP inventory and the CLI/MCP invocation transports.

#![cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]

use cua_driver_testkit::{CliDriver, Driver, McpDriver, RawDriver};

#[test]
fn visual_tool_inventory_advertises_the_versioned_contract() {
    let Some(mut driver) = RawDriver::spawn() else {
        return;
    };
    driver.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
    }));
    driver.recv();
    driver.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}
    }));
    let response = driver.recv();
    let tool = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "parse_visual_regions")
        .expect("parse_visual_regions in tools/list");

    assert_eq!(
        tool["inputSchema"]["required"],
        serde_json::json!(["capture_id"])
    );
    assert_eq!(
        tool["outputSchema"]["properties"]["schema"]["const"],
        "cua.visual_regions_v1"
    );
    assert_eq!(tool["annotations"]["readOnlyHint"], true);
    assert!(tool["capabilities"]
        .as_array()
        .is_some_and(|values| values.iter().any(|value| value == "visual.regions.parse")));

    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../cua-driver-contract/tests/fixtures/parse-visual-regions-output-full-v1.json"
    ))
    .unwrap();
    let validator =
        jsonschema::validator_for(&tool["outputSchema"]).expect("visual output schema compiles");
    assert!(
        validator.is_valid(&fixture),
        "cua.visual_regions_v1 fixture must match the advertised MCP schema"
    );
}

#[test]
fn cli_and_mcp_preserve_the_same_visual_error_dto() {
    let arguments = serde_json::json!({"capture_id": "capture-does-not-exist"});
    let mut cli = CliDriver::new();
    if !cli.available() {
        return;
    }
    let cli_response = cli.call("parse_visual_regions", arguments.clone());
    let Some(mut mcp) = McpDriver::spawn() else {
        return;
    };
    let mcp_response = mcp.call("parse_visual_regions", arguments);

    assert!(cli_response.is_error());
    assert!(mcp_response.is_error());
    assert_eq!(cli_response.structured(), mcp_response.structured());
    assert_eq!(cli_response.structured()["code"], "capture_not_found");
    assert_eq!(cli_response.structured()["retryable"], false);
    let error: cua_driver_contract::VisualParseError =
        serde_json::from_value(cli_response.structured().clone()).unwrap();
    error.validate().unwrap();
}
