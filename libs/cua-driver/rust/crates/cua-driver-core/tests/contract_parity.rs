// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use cua_driver_contract::{manifest, SchemaMode, CAPABILITY_VERSION, TOOLS_LIST_SCHEMA_VERSION};
use cua_driver_core::perception_client::PerceptionClient;
use cua_driver_core::tool::ToolRegistry;

#[test]
fn canonical_core_contracts_match_live_registry() {
    let mut registry = ToolRegistry::new();
    registry.register_session_tools();
    registry.register_perception_tool(PerceptionClient::unavailable());
    let live = registry.tools_list();

    assert_eq!(live["schema_version"], TOOLS_LIST_SCHEMA_VERSION);
    assert_eq!(live["capability_version"], CAPABILITY_VERSION);

    for contract in manifest()
        .tools
        .into_iter()
        .filter(|contract| contract.schema_mode == SchemaMode::CanonicalRuntime)
        .filter(|contract| {
            contract.name == "parse_visual_regions"
                || contract
                    .capabilities
                    .iter()
                    .any(|capability| capability.starts_with("session."))
        })
    {
        let entry = live["tools"]
            .as_array()
            .expect("live tools array")
            .iter()
            .find(|entry| entry["name"] == contract.name)
            .unwrap_or_else(|| panic!("{} missing from live registry", contract.name));
        assert_eq!(entry["description"], contract.description, "description");
        assert_eq!(entry["inputSchema"], contract.input_schema, "inputSchema");
        assert_eq!(
            entry["annotations"]["readOnlyHint"], contract.annotations.read_only,
            "readOnlyHint"
        );
        assert_eq!(
            entry["annotations"]["destructiveHint"], contract.annotations.destructive,
            "destructiveHint"
        );
        assert_eq!(
            entry["annotations"]["idempotentHint"], contract.annotations.idempotent,
            "idempotentHint"
        );
        assert_eq!(
            entry["annotations"]["openWorldHint"], contract.annotations.open_world,
            "openWorldHint"
        );
        assert_eq!(
            entry["capabilities"],
            serde_json::json!(contract.capabilities)
        );
    }
}
