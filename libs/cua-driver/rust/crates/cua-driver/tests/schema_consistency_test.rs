//! Cross-platform tool-contract consistency gate.
//!
//! Spawns the active backend, asks it for `tools/list`, and runs every tool's
//! `inputSchema` through [`cua_driver_core::tool_schema::shared_schema_violations`].
//! It also verifies that every registered tool has a reviewed risk class.
//! Because this test compiles+runs against whichever platform crate is active,
//! the SAME file guards macOS, Linux, and Windows in their respective CI lanes:
//! the moment one platform's hand-written schema drifts from the shared canon
//! (a stale `capture_mode` enum, a `session` param with the wrong shape, a
//! `required` set that diverges), or registers an unclassified tool, this fails.
//!
//! This is the codified form of the manual macOS↔Windows diff that surfaced the
//! drift in the first place. It is NOT `#[ignore]`d — `tools/list` needs no GUI,
//! permissions, or display, so it runs in normal CI.

use cua_driver_contract::{
    compatibility::schema_subset_violations, manifest, Platform, SchemaMode,
};
use cua_driver_core::tool_schema::shared_schema_violations;
use cua_driver_testkit::RawDriver;
use serde_json::{json, Value};

#[test]
fn registered_tool_contracts_match_on_active_backend() {
    let Some(mut driver) = RawDriver::spawn() else {
        // Binary not built — testkit already printed a skip note.
        return;
    };

    // Drive the handshake ourselves (RawDriver does not auto-initialize).
    driver.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "schema-gate", "version": "1" }
        }
    }));
    let _ = driver.recv();

    driver.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let resp = driver.recv();

    let tools = resp["result"]["tools"]
        .as_array()
        .expect("tools/list must return result.tools");
    assert!(!tools.is_empty(), "tools/list returned no tools");

    let mut violations: Vec<String> = Vec::new();
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or("<unnamed>");
        match tool.pointer("/risk/class").and_then(|value| value.as_str()) {
            Some("unclassified") => {
                violations.push(format!("{name}: risk class is unclassified"));
            }
            Some(_) => {}
            None => violations.push(format!("{name}: risk class is missing")),
        }

        // MCP standard key is `inputSchema`; accept the snake_case fallback too.
        let schema = tool
            .get("inputSchema")
            .or_else(|| tool.get("input_schema"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        violations.extend(shared_schema_violations(name, &schema));
    }

    assert!(
        violations.is_empty(),
        "tool-contract drift on this backend ({} violation(s)):\n  {}",
        violations.len(),
        violations.join("\n  ")
    );
}

#[test]
fn portable_desktop_contracts_are_accepted_by_active_backend() {
    let Some(mut driver) = RawDriver::spawn() else {
        // Binary not built — testkit already printed a skip note.
        return;
    };

    driver.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "portable-contract-gate", "version": "1" }
        }
    }));
    let _ = driver.recv();

    driver.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let response = driver.recv();
    let live_tools = response["result"]["tools"]
        .as_array()
        .expect("tools/list must return result.tools");

    let active_platform = if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else {
        panic!("portable contract parity is unsupported on this target")
    };

    let mut violations = Vec::new();
    for contract in manifest()
        .tools
        .into_iter()
        .filter(|contract| contract.schema_mode == SchemaMode::PortableSubset)
        .filter(|contract| contract.platforms.contains(&active_platform))
    {
        let Some(live) = live_tools
            .iter()
            .find(|entry| entry["name"].as_str() == Some(&contract.name))
        else {
            violations.push(format!("{}: missing from live registry", contract.name));
            continue;
        };

        let live_schema = live
            .get("inputSchema")
            .or_else(|| live.get("input_schema"))
            .unwrap_or(&Value::Null);
        violations.extend(
            schema_subset_violations(&contract.input_schema, live_schema)
                .into_iter()
                .map(|violation| format!("{}: {violation}", contract.name)),
        );

        let annotations = &live["annotations"];
        for (name, actual, expected) in [
            (
                "readOnlyHint",
                annotations["readOnlyHint"].as_bool(),
                contract.annotations.read_only,
            ),
            (
                "destructiveHint",
                annotations["destructiveHint"].as_bool(),
                contract.annotations.destructive,
            ),
            (
                "idempotentHint",
                annotations["idempotentHint"].as_bool(),
                contract.annotations.idempotent,
            ),
            (
                "openWorldHint",
                annotations["openWorldHint"].as_bool(),
                contract.annotations.open_world,
            ),
        ] {
            if actual != Some(expected) {
                violations.push(format!(
                    "{}: annotation {name} is {actual:?}, expected {expected}",
                    contract.name
                ));
            }
        }

        if live["capabilities"] != json!(contract.capabilities) {
            violations.push(format!(
                "{}: live capabilities {} differ from portable capabilities {}",
                contract.name,
                live["capabilities"],
                json!(contract.capabilities)
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "portable SDK contracts are not accepted by the active backend ({} violation(s)):\n  {}",
        violations.len(),
        violations.join("\n  ")
    );
}

#[test]
fn health_report_is_callable_through_authorized_mcp_surface() {
    let Some(mut driver) = RawDriver::spawn() else {
        // Binary not built — testkit already printed a skip note.
        return;
    };

    driver.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "health-report-risk-gate", "version": "1" }
        }
    }));
    let _ = driver.recv();

    driver.send(&json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "health_report", "arguments": {} }
    }));
    let resp = driver.recv();
    let result = &resp["result"];

    assert_ne!(
        result["isError"].as_bool(),
        Some(true),
        "health_report must remain callable under the standard risk gate: {resp:?}"
    );
    assert_eq!(
        result["structuredContent"]["schema_version"], "1",
        "health_report must preserve its stable schema_version=1 contract: {resp:?}"
    );
}
