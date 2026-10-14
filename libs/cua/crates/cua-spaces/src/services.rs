//! Tools inside a Space, from two kinds of MCP service:
//!
//! - `driver`: the spacesd's cua-driver registry over `DriverService`
//!   (the same registry the driver serves as MCP at `/mcp`), when the image
//!   runs cua-spacesd;
//! - any service the Space declares (`mcp`, `blender`, ...): the official
//!   Rust MCP client (rmcp) over the service pipe (loopback locally, the
//!   Fleet gateway on Fleet, the URL of a Space added by its MCP endpoint).
//!   Results are forwarded as the server sent them (every content block,
//!   `structuredContent`, `_meta`). No spacesd needed.

use crate::error::{Error, Result};
use crate::space::Space;
use cua_sandbox_core::McpClient;
use cua_sandbox_core::mcp::rmcp;
use cua_spacesd_client::pb;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Duration;

/// The service name of the spacesd's own tool registry.
pub const DRIVER_SERVICE: &str = "driver";

/// Names that mean "the Space's own cua-driver" unless the Space declares a
/// service by that name: `mcp` (Fleet), `cua-driver` (the guest registry),
/// `computer-server` (the Python server's default), and the default (empty).
pub const DRIVER_ALIASES: [&str; 5] = ["driver", "mcp", "cua-driver", "computer-server", ""];

/// A tool description.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ToolInfo {
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Input JSON Schema.
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    /// Output JSON Schema, when declared.
    #[serde(rename = "outputSchema", skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// Does not modify guest state.
    pub read_only: bool,
    /// May perform destructive changes.
    pub destructive: bool,
}

/// A tool call's result, as MCP content parts.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ToolResult {
    /// MCP content parts (`text`, `image`), images base64-encoded.
    pub content: Vec<Value>,
    /// Structured result, when the tool declares an output schema.
    #[serde(rename = "structuredContent", skip_serializing_if = "Option::is_none")]
    pub structured: Option<Value>,
    /// The tool reported a failure.
    #[serde(rename = "isError")]
    pub is_error: bool,
    /// The result's `_meta`, when present.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Where a `service` argument leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceRoute {
    /// The spacesd's cua-driver registry.
    Driver,
    /// A declared service, over generic MCP.
    Mcp(String),
}

impl ServiceRoute {
    /// The service name to report.
    pub fn name(&self) -> &str {
        match self {
            ServiceRoute::Driver => DRIVER_SERVICE,
            ServiceRoute::Mcp(s) => s,
        }
    }
}

fn parse_json(text: &str) -> Value {
    if text.trim().is_empty() {
        return Value::Null;
    }
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.into()))
}

impl Space {
    /// The MCP services this Space exposes: `driver` when its spacesd has
    /// the tool registry, then every declared service.
    pub fn services(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.supports("driver") {
            out.push(DRIVER_SERVICE.to_string());
        }
        out.extend(self.declared_services().keys().cloned());
        out
    }

    /// Resolves a `service` argument: a declared service by its name wins;
    /// otherwise `driver` and its aliases (and no service) mean the
    /// spacesd registry; with no spacesd, no service means the only
    /// declared service (or `mcp`).
    pub fn resolve_service(&self, service: Option<&str>) -> Result<ServiceRoute> {
        let s = service.unwrap_or("").trim();
        let declared = self.declared_services();
        if !s.is_empty() && declared.contains_key(s) {
            return Ok(ServiceRoute::Mcp(s.into()));
        }
        if DRIVER_ALIASES.contains(&s) {
            if self.has_spacesd() {
                self.require("driver")?;
                return Ok(ServiceRoute::Driver);
            }
            if s.is_empty() {
                if declared.len() == 1 {
                    return Ok(ServiceRoute::Mcp(
                        declared.keys().next().cloned().unwrap_or_default(),
                    ));
                }
                if declared.contains_key("mcp") {
                    return Ok(ServiceRoute::Mcp("mcp".into()));
                }
            }
            if declared.is_empty() || s == DRIVER_SERVICE || s == "cua-driver" {
                // The same answer a driver without the registry gives.
                self.require("driver")?;
            }
        }
        Err(Error::NotFound(format!(
            "MCP service {s:?} in {} (it exposes: {})",
            self.id(),
            match self.services() {
                v if v.is_empty() => "no services".to_string(),
                v => v.join(", "),
            }
        )))
    }

    /// A connected MCP client (rmcp) for declared service `service`: one
    /// connection per service, shared by every clone of this handle and
    /// reopened when it closed.
    pub async fn mcp(&self, service: &str) -> Result<Arc<McpClient>> {
        let svc = self.declared_services().get(service).ok_or_else(|| {
            Error::NotFound(format!(
                "service {service:?} in {} (declared: {:?})",
                self.id(),
                self.declared_services().keys().collect::<Vec<_>>()
            ))
        })?;
        let mut cache = self.inner.mcp.lock().await;
        if let Some(c) = cache.get(service).filter(|c| !c.is_closed()) {
            return Ok(c.clone());
        }
        let client = Arc::new(svc.mcp_config().await?.connect().await?);
        cache.insert(service.to_string(), client.clone());
        Ok(client)
    }

    async fn drop_mcp(&self, service: &str) {
        self.inner.mcp.lock().await.remove(service);
    }

    /// Tools of `service` (default: the driver registry, or the only
    /// declared service of a Space without spacesd). Returns the tools
    /// and the registry's contract version (`mcp/<revision>` for a generic
    /// MCP service).
    pub async fn list_tools(&self, service: Option<&str>) -> Result<(Vec<ToolInfo>, String)> {
        match self.resolve_service(service)? {
            ServiceRoute::Driver => self.list_driver_tools().await,
            ServiceRoute::Mcp(name) => {
                let client = self.mcp(&name).await?;
                let tools = match client.peer().list_all_tools().await {
                    Ok(t) => t,
                    Err(e) => {
                        self.drop_mcp(&name).await;
                        return Err(Error::Mcp(e.to_string()));
                    }
                };
                let version = client
                    .peer_info()
                    .map(|i| i.protocol_version.to_string())
                    .unwrap_or_default();
                let tools = tools
                    .into_iter()
                    .map(|t| {
                        let v = serde_json::to_value(&t).unwrap_or(Value::Null);
                        let hint = |k: &str| {
                            v.pointer(&format!("/annotations/{k}"))
                                .and_then(Value::as_bool)
                        };
                        let read_only = hint("readOnlyHint").unwrap_or(false);
                        ToolInfo {
                            name: t.name.to_string(),
                            description: t.description.as_deref().unwrap_or_default().to_string(),
                            input_schema: v
                                .get("inputSchema")
                                .cloned()
                                .unwrap_or(json!({"type": "object"})),
                            output_schema: v.get("outputSchema").cloned(),
                            read_only,
                            destructive: hint("destructiveHint").unwrap_or(!read_only),
                        }
                    })
                    .collect();
                Ok((tools, format!("mcp/{version}")))
            }
        }
    }

    async fn list_driver_tools(&self) -> Result<(Vec<ToolInfo>, String)> {
        let resp = self
            .spacesd()?
            .driver()
            .list_tools(pb::ListToolsRequest {})
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        let tools = resp
            .tools
            .into_iter()
            .map(|t| ToolInfo {
                name: t.name,
                description: t.description,
                input_schema: match parse_json(&t.input_schema_json) {
                    Value::Null => json!({"type": "object", "properties": {}}),
                    v => v,
                },
                output_schema: match parse_json(&t.output_schema_json) {
                    Value::Null => None,
                    v => Some(v),
                },
                read_only: t.read_only,
                destructive: t.destructive,
            })
            .collect();
        Ok((tools, resp.contract_version))
    }

    /// Calls `tool` on `service` (see [`Space::list_tools`]). A tool-level
    /// failure is a result with `is_error`, not an error.
    pub async fn call_tool(
        &self,
        service: Option<&str>,
        tool: &str,
        arguments: Map<String, Value>,
        timeout: Option<Duration>,
    ) -> Result<ToolResult> {
        match self.resolve_service(service)? {
            ServiceRoute::Driver => {
                let resp = self
                    .spacesd()?
                    .driver()
                    .call_tool(pb::CallToolRequest {
                        name: tool.into(),
                        arguments_json: Value::Object(arguments).to_string(),
                        timeout: timeout.map(|d| pbjson_types::Duration {
                            seconds: d.as_secs() as i64,
                            nanos: d.subsec_nanos() as i32,
                        }),
                    })
                    .await
                    .map_err(cua_spacesd_client::Error::from)?
                    .into_inner();
                Ok(tool_result(resp))
            }
            ServiceRoute::Mcp(name) => {
                let client = self.mcp(&name).await?;
                let params = rmcp::model::CallToolRequestParams::new(tool.to_string())
                    .with_arguments(arguments);
                let call = client.peer().call_tool(params);
                let r = match timeout {
                    Some(t) => tokio::time::timeout(t, call)
                        .await
                        .map_err(|_| Error::Timeout(format!("{name}/{tool} after {t:?}")))?,
                    None => call.await,
                };
                let r = match r {
                    Ok(r) => r,
                    Err(e) => {
                        if client.is_closed() {
                            self.drop_mcp(&name).await;
                        }
                        return Err(Error::Mcp(e.to_string()));
                    }
                };
                Ok(ToolResult::from_mcp(serde_json::to_value(&r)?))
            }
        }
    }
}

impl ToolResult {
    /// A `tools/call` result as the MCP server sent it: content blocks,
    /// `structuredContent`, `isError` and `_meta` kept verbatim.
    pub fn from_mcp(mut raw: Value) -> Self {
        let content = match raw.get_mut("content").map(Value::take) {
            Some(Value::Array(a)) => a,
            _ => vec![],
        };
        ToolResult {
            content,
            structured: raw.get_mut("structuredContent").map(Value::take),
            is_error: raw.get("isError").and_then(Value::as_bool).unwrap_or(false),
            meta: raw.get_mut("_meta").map(Value::take),
        }
    }
}

/// Converts a `CallToolResponse` into MCP content parts, preserving images.
pub fn tool_result(resp: pb::CallToolResponse) -> ToolResult {
    use base64::Engine;
    let mut content = Vec::new();
    if resp.is_error {
        content.push(json!({"type": "text", "text": "[in-space tool reported an error]"}));
    }
    for part in resp.content {
        match part.content {
            Some(pb::tool_content::Content::Text(t)) => {
                content.push(json!({"type": "text", "text": t}));
            }
            Some(pb::tool_content::Content::Json(j)) => {
                content.push(json!({"type": "text", "text": j}));
            }
            Some(pb::tool_content::Content::Image(img)) => content.push(json!({
                "type": "image",
                "data": base64::engine::general_purpose::STANDARD.encode(&img.data),
                "mimeType": if img.mime_type.is_empty() { "image/png".to_string() } else { img.mime_type },
            })),
            None => {}
        }
    }
    let structured = match parse_json(&resp.structured_json) {
        Value::Null => None,
        v => Some(v),
    };
    if content.is_empty() {
        content.push(json!({
            "type": "text",
            "text": structured.as_ref().map(|s| s.to_string()).unwrap_or_else(|| "[no output]".into()),
        }));
    }
    ToolResult {
        content,
        structured,
        is_error: resp.is_error,
        meta: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_survive_as_mcp_image_parts() {
        let r = tool_result(pb::CallToolResponse {
            content: vec![
                pb::ToolContent {
                    content: Some(pb::tool_content::Content::Text("captured".into())),
                },
                pb::ToolContent {
                    content: Some(pb::tool_content::Content::Image(pb::ToolImage {
                        data: vec![1, 2, 3],
                        mime_type: "image/png".into(),
                    })),
                },
            ],
            structured_json: String::new(),
            is_error: false,
        });
        assert_eq!(r.content[1]["type"], "image");
        assert_eq!(r.content[1]["data"], "AQID");
        assert!(!r.is_error);
    }

    #[test]
    fn a_tool_error_is_flagged_not_thrown() {
        let r = tool_result(pb::CallToolResponse {
            content: vec![],
            structured_json: String::new(),
            is_error: true,
        });
        assert!(r.is_error);
        assert!(
            r.content[0]["text"]
                .as_str()
                .unwrap()
                .contains("reported an error")
        );
    }
}
