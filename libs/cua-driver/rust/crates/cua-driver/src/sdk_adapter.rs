//! Transport-facing adapter over the public Cua Driver SDK contract.
//!
//! The daemon, MCP transports, and finite CLI inspection commands all consume
//! this adapter. Platform registries remain private to `cua-driver-sdk`.

use std::sync::Arc;

use cua_driver_core::server::ToolProvider;
use cua_driver_sdk::CuaDriver;
use serde_json::{json, Value};

pub struct SdkAdapter {
    driver: Arc<CuaDriver>,
    tools_list: Value,
}

impl SdkAdapter {
    pub async fn load(driver: Arc<CuaDriver>) -> anyhow::Result<Arc<Self>> {
        let tools_json = driver
            .list_tools_json()
            .await
            .map_err(|error| anyhow::anyhow!("load SDK tool inventory: {error}"))?;
        let tools_list: Value = serde_json::from_str(&tools_json)
            .map_err(|error| anyhow::anyhow!("parse SDK tool inventory: {error}"))?;
        if !tools_list.get("tools").is_some_and(Value::is_array) {
            anyhow::bail!("SDK tool inventory omitted tools array");
        }
        Ok(Arc::new(Self { driver, tools_list }))
    }

    pub fn load_blocking(driver: Arc<CuaDriver>) -> anyhow::Result<Arc<Self>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(Self::load(driver))
    }

    pub fn tools_list(&self) -> Value {
        self.tools_list.clone()
    }

    pub fn is_known_tool(&self, name: &str) -> bool {
        name == "type_text_chars"
            || self
                .tools_list
                .get("tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| {
                    tools
                        .iter()
                        .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
                })
    }

    pub fn describe(&self, name: &str) -> Option<Value> {
        self.tools_list
            .get("tools")?
            .as_array()?
            .iter()
            .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
            .map(|tool| {
                json!({
                    "name": tool.get("name").cloned().unwrap_or(Value::Null),
                    "description": tool.get("description").cloned().unwrap_or(Value::String(String::new())),
                    "input_schema": tool.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object"})),
                })
            })
    }

    /// Legacy daemon inventory shape consumed by released socket clients.
    pub fn daemon_tools_list(&self) -> Value {
        daemon_tools_list_from(&self.tools_list)
    }

    pub async fn invoke_raw(&self, name: &str, arguments: Value) -> Result<Value, String> {
        let arguments_json =
            serde_json::to_string(&arguments).map_err(|error| error.to_string())?;
        let result = self
            .driver
            .call_tool(name.to_owned(), arguments_json)
            .await
            .map_err(|error| error.to_string())?;
        serde_json::from_str(&result.raw_json)
            .map_err(|error| format!("{name} returned invalid SDK result JSON: {error}"))
    }

    pub async fn end_session(&self, session: &str) -> Result<(), String> {
        self.invoke_raw("end_session", json!({"session": session}))
            .await
            .map(|_| ())
    }

    pub async fn revoke_all_sessions(&self) -> Result<usize, String> {
        self.driver
            .revoke_all_host_sessions()
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        self.driver
            .shutdown()
            .await
            .map_err(|error| error.to_string())
    }
}

fn daemon_tools_list_from(tools_list: &Value) -> Value {
    let tools = tools_list
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|tool| {
                let annotations = tool.get("annotations").unwrap_or(&Value::Null);
                json!({
                    "name": tool.get("name").cloned().unwrap_or(Value::Null),
                    "description": tool.get("description").cloned().unwrap_or(Value::String(String::new())),
                    "input_schema": tool.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object"})),
                    "read_only": annotations.get("readOnlyHint").cloned().unwrap_or(Value::Bool(false)),
                    "destructive": annotations.get("destructiveHint").cloned().unwrap_or(Value::Bool(false)),
                    "idempotent": annotations.get("idempotentHint").cloned().unwrap_or(Value::Bool(false)),
                    "open_world": annotations.get("openWorldHint").cloned().unwrap_or(Value::Bool(false)),
                    "capabilities": tool.get("capabilities").cloned().unwrap_or_else(|| json!([])),
                    "risk": tool.get("risk").cloned().unwrap_or(Value::Null),
                })
            })
            .collect::<Vec<_>>();
    json!({
        "tools": tools,
        "capability_version": tools_list.get("capability_version").cloned().unwrap_or(Value::Null),
        "schema_version": tools_list.get("schema_version").cloned().unwrap_or(Value::Null),
        "tool_observation_owner": "daemon",
    })
}

#[async_trait::async_trait]
impl ToolProvider for SdkAdapter {
    fn tools_list(&self) -> Value {
        self.tools_list()
    }

    async fn invoke_tool(&self, name: &str, arguments: Value) -> Result<Value, String> {
        self.invoke_raw(name, arguments).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_shape_is_derived_from_canonical_mcp_inventory() {
        let tools_list = json!({
            "tools": [{
                "name": "probe",
                "description": "Probe.",
                "inputSchema": {"type": "object"},
                "annotations": {
                    "readOnlyHint": true,
                    "destructiveHint": false,
                    "idempotentHint": true,
                    "openWorldHint": false
                },
                "capabilities": ["probe.read"],
                "risk": {"level": "low"}
            }],
            "capability_version": "1",
            "schema_version": "1"
        });
        let daemon = daemon_tools_list_from(&tools_list);
        assert_eq!(daemon["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(daemon["tools"][0]["read_only"], true);
        assert_eq!(daemon["tool_observation_owner"], "daemon");
    }
}
