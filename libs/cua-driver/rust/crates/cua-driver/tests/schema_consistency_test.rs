//! Cross-platform tool-schema consistency gate.
//!
//! Spawns the active backend, asks it for `tools/list`, and runs every tool's
//! `inputSchema` through [`cua_driver_core::tool_schema::shared_schema_violations`].
//! Because this test compiles+runs against whichever platform crate is active,
//! the SAME file guards macOS, Linux, and Windows in their respective CI lanes:
//! the moment one platform's hand-written schema drifts from the shared canon
//! (a stale `capture_mode` enum, a `session` param with the wrong shape, a
//! `required` set that diverges), this fails.
//!
//! This is the codified form of the manual macOS↔Windows diff that surfaced the
//! drift in the first place. It is NOT `#[ignore]`d — `tools/list` needs no GUI,
//! permissions, or display, so it runs in normal CI.

use cua_driver_core::tool_schema::shared_schema_violations;
use cua_driver_testkit::RawDriver;
use serde_json::json;

#[test]
fn shared_tool_params_match_canon_on_active_backend() {
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
        "shared-param schema drift on this backend ({} violation(s)):\n  {}",
        violations.len(),
        violations.join("\n  ")
    );
}
