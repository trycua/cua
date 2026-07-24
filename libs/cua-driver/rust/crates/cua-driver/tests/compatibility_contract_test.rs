//! Compatibility lock for selected stable CLI and MCP fields from 0.12.6.

use std::collections::HashSet;
use std::process::Command;

use cua_driver_testkit::RawDriver;
use serde_json::{json, Value};

const CLI_FIXTURE: &str = include_str!("../../../../compat-fixtures/cli.json");
const MCP_FIXTURE: &str = include_str!("../../../../compat-fixtures/mcp.json");

#[test]
fn released_cli_help_and_manifest_fields_remain_compatible() {
    let fixture: Value = serde_json::from_str(CLI_FIXTURE).expect("valid CLI fixture");
    let executable = env!("CARGO_BIN_EXE_cua-driver");

    let help = Command::new(executable)
        .arg("--help")
        .output()
        .expect("run cua-driver --help");
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("UTF-8 help");
    for line in fixture["help_lines"].as_array().expect("help lines") {
        let line = line.as_str().expect("help line string");
        assert!(help.contains(line), "CLI help no longer contains {line:?}");
    }

    let manifest = Command::new(executable)
        .arg("manifest")
        .output()
        .expect("run cua-driver manifest");
    assert!(manifest.status.success());
    let manifest: Value = serde_json::from_slice(&manifest.stdout).expect("JSON CLI manifest");
    let expected = &fixture["manifest"];
    assert_eq!(manifest["schema_version"], expected["schema_version"]);
    semver::Version::parse(
        manifest["binary_version"]
            .as_str()
            .expect("binary_version string"),
    )
    .expect("binary_version remains semantic version");
    assert_eq!(manifest["mcp_invocation"]["args"], expected["mcp_args"]);
    assert!(
        manifest["mcp_invocation"]["command"]
            .as_str()
            .is_some_and(|command| !command.is_empty()),
        "MCP command remains a non-empty executable path"
    );

    let actual_subcommands = manifest["subcommands"]
        .as_array()
        .expect("subcommands array");
    for (name, expected_args) in expected["subcommands"]
        .as_object()
        .expect("fixture subcommands")
    {
        let actual = actual_subcommands
            .iter()
            .find(|subcommand| subcommand["name"] == name.as_str())
            .unwrap_or_else(|| panic!("manifest no longer contains subcommand {name}"));
        let actual_args: HashSet<(&str, &str)> = actual["args"]
            .as_array()
            .expect("subcommand args")
            .iter()
            .map(|argument| {
                (
                    argument["name"].as_str().expect("argument name"),
                    argument["type"].as_str().expect("argument type"),
                )
            })
            .collect();
        for expected_arg in expected_args.as_array().expect("fixture args") {
            let pair = expected_arg.as_array().expect("fixture argument pair");
            let expected_pair = (
                pair[0].as_str().expect("fixture argument name"),
                pair[1].as_str().expect("fixture argument type"),
            );
            assert!(
                actual_args.contains(&expected_pair),
                "{name} no longer contains argument {expected_pair:?}"
            );
        }
    }
}

#[test]
fn released_mcp_initialize_tools_list_and_error_fields_remain_compatible() {
    let fixture: Value = serde_json::from_str(MCP_FIXTURE).expect("valid MCP fixture");
    let Some(mut driver) = RawDriver::spawn() else {
        panic!("compatibility test requires the built cua-driver binary");
    };
    assert_mcp_contract(&mut driver, &fixture);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn direct_mcp_runtime_matches_the_released_protocol_contract() {
    let fixture: Value = serde_json::from_str(MCP_FIXTURE).expect("valid MCP fixture");
    let Some(mut driver) = RawDriver::spawn_direct() else {
        panic!("compatibility test requires the built cua-driver binary");
    };
    assert_mcp_contract(&mut driver, &fixture);
}

fn assert_mcp_contract(driver: &mut RawDriver, fixture: &Value) {
    driver.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "clientInfo": {"name": "compatibility-test", "version": "0"}
        }
    }));
    let initialize = driver.recv();
    let expected_initialize = &fixture["initialize"];
    assert_eq!(initialize["jsonrpc"], expected_initialize["jsonrpc"]);
    assert_eq!(
        initialize["result"]["protocolVersion"],
        expected_initialize["protocol_version"]
    );
    assert_eq!(
        initialize["result"]["serverInfo"]["name"],
        expected_initialize["server_name"]
    );
    semver::Version::parse(
        initialize["result"]["serverInfo"]["version"]
            .as_str()
            .expect("server version string"),
    )
    .expect("server version remains semantic version");
    for capability in expected_initialize["capability_keys"]
        .as_array()
        .expect("capability keys")
    {
        let capability = capability.as_str().expect("capability string");
        assert!(
            initialize["result"]["capabilities"]
                .get(capability)
                .is_some(),
            "initialize no longer advertises {capability}"
        );
    }

    driver.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let tools_list = driver.recv();
    let expected_tools = &fixture["tools_list"];
    assert_eq!(
        tools_list["result"]["schema_version"],
        expected_tools["schema_version"]
    );
    assert_eq!(
        tools_list["result"]["capability_version"],
        expected_tools["capability_version"]
    );
    let tools = tools_list["result"]["tools"]
        .as_array()
        .expect("tools/list tools array");
    for name in expected_tools["required_tools"]
        .as_array()
        .expect("required tools")
    {
        assert!(
            tools.iter().any(|tool| tool["name"] == *name),
            "tools/list no longer contains {name}"
        );
    }
    for (name, fields) in expected_tools["selected_tool_fields"]
        .as_object()
        .expect("selected tool fields")
    {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == name.as_str())
            .unwrap_or_else(|| panic!("tools/list no longer contains {name}"));
        for field in fields.as_array().expect("tool fields") {
            let field = field.as_str().expect("tool field string");
            assert!(tool.get(field).is_some(), "{name} no longer has {field}");
        }
        assert_eq!(tool["inputSchema"]["type"], "object");
    }

    driver.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "compatibility/unknown"
    }));
    let error = driver.recv();
    assert_eq!(
        error["error"]["code"],
        fixture["unknown_method_error"]["code"]
    );
    assert_eq!(
        error["error"]["message"],
        fixture["unknown_method_error"]["message"]
    );

    driver.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {"name": "compatibility_unknown_tool", "arguments": {}}
    }));
    let tool_call = driver.recv();
    assert_eq!(tool_call["jsonrpc"], "2.0");
    for field in fixture["tool_call"]["required_result_fields"]
        .as_array()
        .expect("tool result field fixture")
    {
        let field = field.as_str().expect("tool result field string");
        assert!(
            tool_call["result"].get(field).is_some(),
            "tools/call result no longer contains {field}"
        );
    }
}
