// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The registry a platform crate builds when compiled for another OS.
//!
//! Workspace-wide checks compile every platform crate on every host. Off its
//! own OS a platform crate advertises the published contract tools for its
//! platform, each refusing with a "not implemented" error, plus the
//! platform-independent session and recording tools.

use async_trait::async_trait;
use cua_driver_contract::Platform;
use serde_json::Value;

use crate::protocol::ToolResult;
use crate::tool::{Tool, ToolDef, ToolRegistry};

struct UnsupportedTool {
    def: ToolDef,
    platform: &'static str,
}

#[async_trait]
impl Tool for UnsupportedTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        ToolResult::error(format!(
            "{}: {} implementation is only available when compiled for {}.",
            self.def.name, self.platform, self.platform
        ))
    }
}

/// Build the registry for `platform` on a host that cannot run it. `label`
/// names the platform in each tool's refusal.
pub fn registry(platform: Platform, label: &'static str) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_recording_tools();
    registry.register_session_tools();
    for contract in cua_driver_contract::manifest().tools {
        if !contract.platforms.contains(&platform) || registry.get_def(&contract.name).is_some() {
            continue;
        }
        // Not `ToolDef::from_contract`: that bridge refuses portable subset
        // contracts, and these tools only advertise the published schema.
        registry.register(Box::new(UnsupportedTool {
            def: ToolDef {
                name: contract.name,
                description: contract.description,
                input_schema: contract.input_schema,
                read_only: contract.annotations.read_only,
                destructive: contract.annotations.destructive,
                idempotent: contract.annotations.idempotent,
                open_world: contract.annotations.open_world,
            },
            platform: label,
        }));
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn advertises_contract_tools_that_refuse() {
        let registry = registry(Platform::Linux, "Linux");
        let list = registry.tools_list();
        let names: Vec<&str> = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        for contract in cua_driver_contract::manifest().tools {
            assert_eq!(
                names.iter().filter(|name| **name == contract.name).count(),
                1,
                "{}",
                contract.name
            );
        }
        assert!(!names.contains(&"type_text_chars"));
        let result = registry.invoke("list_apps", serde_json::json!({})).await;
        assert_eq!(result.is_error, Some(true));
        assert!(
            serde_json::to_string(&result).unwrap().contains(
                "list_apps: Linux implementation is only available when compiled for Linux."
            ),
            "{result:?}"
        );
    }
}
