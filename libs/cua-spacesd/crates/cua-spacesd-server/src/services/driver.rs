// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `DriverService`: passthrough to the in-process cua-driver tool registry,
//! dispatched through cua-driver's own MCP handler so authorization and
//! argument handling match `/mcp` and `cua-driver mcp` exactly.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use cua_driver_core::protocol::Request as RpcRequest;
use cua_driver_core::server::{handle_request_with_transport_session, ToolProvider};
use cua_proto::env::v1::driver_service_server::{DriverService, DriverServiceServer};
use cua_proto::env::v1::tool_content::Content;
use cua_proto::env::v1::*;
use serde_json::{json, Value};
use tonic::{Code, Request, Response, Status};

use crate::auth::caller;
use crate::error::status;

/// Limitation reported when no registry is linked.
pub const NO_REGISTRY: &str = "this cua-spacesd build has no cua-driver tool registry";

/// How long a driver session may sit unused before it ends (the documented
/// five minutes; `CUA_DRIVER_RS_SESSION_IDLE_TTL_SECS` overrides it, as it
/// does for `cua-driver mcp`).
const SESSION_IDLE_TTL: Duration = Duration::from_secs(300);
/// How often idle sessions are looked for.
const SESSION_SWEEP_EVERY: Duration = Duration::from_secs(30);

/// Ends driver sessions idle past `ttl`, every `every`, until the task is
/// dropped. The standalone driver and `DriverRuntime` run this sweep; the
/// registry cua-spacesd builds does not go through either, so without it a
/// session (its element tokens, its recording) outlived the documented TTL.
pub fn spawn_idle_sweep(every: Duration, ttl: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await;
        loop {
            tick.tick().await;
            // Session end hooks can block (they stop recordings): not on the
            // async workers.
            let ended =
                tokio::task::spawn_blocking(move || cua_driver_core::session::evict_idle(ttl))
                    .await
                    .unwrap_or_default();
            if !ended.is_empty() {
                tracing::info!(
                    count = ended.len(),
                    "ended driver sessions idle past their TTL"
                );
            }
        }
    })
}

/// Starts the process-wide sweep once, when a service with a registry is
/// built inside a runtime.
fn start_idle_sweep() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    STARTED.call_once(|| {
        let ttl = std::env::var("CUA_DRIVER_RS_SESSION_IDLE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .map_or(SESSION_IDLE_TTL, Duration::from_secs);
        drop(spawn_idle_sweep(SESSION_SWEEP_EVERY, ttl));
    });
}

/// The gRPC service.
#[derive(Clone)]
pub struct DriverServiceImpl {
    tools: Option<Arc<dyn ToolProvider>>,
}

impl DriverServiceImpl {
    /// Creates the service over an optional registry.
    pub fn new(tools: Option<Arc<dyn ToolProvider>>) -> Self {
        if tools.is_some() {
            start_idle_sweep();
        }
        Self { tools }
    }

    /// Tonic server.
    pub fn into_server(self) -> DriverServiceServer<Self> {
        DriverServiceServer::new(self)
            .max_decoding_message_size(crate::config::MAX_MESSAGE_BYTES as usize)
            .max_encoding_message_size(64 * 1024 * 1024)
    }

    fn tools(&self) -> Result<&Arc<dyn ToolProvider>, Status> {
        self.tools
            .as_ref()
            .ok_or_else(|| crate::error::unsupported("driver", NO_REGISTRY))
    }
}

fn schema_text(value: Option<&Value>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

/// Converts the registry's `tools/list` JSON into `ToolInfo`s.
pub fn tool_infos(list: &Value) -> Vec<ToolInfo> {
    list.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .map(|tool| {
                    let annotations = tool.get("annotations");
                    let flag = |key: &str| {
                        annotations
                            .and_then(|a| a.get(key))
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                    };
                    ToolInfo {
                        name: tool["name"].as_str().unwrap_or_default().to_owned(),
                        description: tool["description"].as_str().unwrap_or_default().to_owned(),
                        input_schema_json: schema_text(tool.get("inputSchema")),
                        output_schema_json: schema_text(tool.get("outputSchema")),
                        read_only: flag("readOnlyHint"),
                        destructive: flag("destructiveHint"),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Converts an MCP tool result into a `CallToolResponse`.
pub fn call_response(result: &Value) -> CallToolResponse {
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                    Some("text") => Some(Content::Text(
                        part["text"].as_str().unwrap_or_default().to_owned(),
                    )),
                    Some("image") => {
                        let data = base64::engine::general_purpose::STANDARD
                            .decode(part["data"].as_str().unwrap_or_default())
                            .unwrap_or_default();
                        Some(Content::Image(ToolImage {
                            data,
                            mime_type: part["mimeType"].as_str().unwrap_or("image/png").to_owned(),
                        }))
                    }
                    Some(_) => Some(Content::Json(part.to_string())),
                    None => None,
                })
                .map(|c| ToolContent { content: Some(c) })
                .collect()
        })
        .unwrap_or_default();
    CallToolResponse {
        content,
        structured_json: result
            .get("structuredContent")
            .filter(|v| !v.is_null())
            .map(Value::to_string)
            .unwrap_or_default(),
        is_error: result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

#[tonic::async_trait]
impl DriverService for DriverServiceImpl {
    async fn list_tools(
        &self,
        _request: Request<ListToolsRequest>,
    ) -> Result<Response<ListToolsResponse>, Status> {
        let list = self.tools()?.tools_list();
        Ok(Response::new(ListToolsResponse {
            tools: tool_infos(&list),
            contract_version: list
                .get("capability_version")
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                })
                .unwrap_or_default(),
        }))
    }

    async fn call_tool(
        &self,
        request: Request<CallToolRequest>,
    ) -> Result<Response<CallToolResponse>, Status> {
        let principal = caller(&request).principal;
        let body = request.into_inner();
        let tools = self.tools()?.clone();
        let list = tools.tools_list();
        let known = list
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| tools.iter().any(|t| t["name"] == body.name.as_str()));
        if !known {
            return Err(status(
                Code::NotFound,
                ErrorReason::Unspecified,
                format!("unknown tool {:?}", body.name),
            ));
        }
        let arguments: Value = if body.arguments_json.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&body.arguments_json)
                .map_err(|e| crate::error::invalid(format!("arguments_json: {e}")))?
        };
        if !arguments.is_object() {
            return Err(crate::error::invalid(
                "arguments_json must be a JSON object",
            ));
        }
        let rpc: RpcRequest = serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": body.name, "arguments": arguments },
        }))
        .map_err(|e| crate::error::internal(e.to_string()))?;
        let session = format!(
            "grpc-{}",
            principal
                .map(|p| p.id)
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| "anonymous".into())
        );
        let call = handle_request_with_transport_session(rpc, json!(1), tools.as_ref(), &session);
        let response = match crate::util::duration(body.timeout.as_ref()).filter(|d| !d.is_zero()) {
            Some(limit) => tokio::time::timeout(limit, call).await.map_err(|_| {
                status(
                    Code::DeadlineExceeded,
                    ErrorReason::Unspecified,
                    format!("tool {:?} did not finish within {limit:?}", body.name),
                )
            })?,
            None => tokio::time::timeout(Duration::from_secs(24 * 3600), call)
                .await
                .map_err(|_| crate::error::internal("tool call timed out"))?,
        };
        let value =
            serde_json::to_value(&response).map_err(|e| crate::error::internal(e.to_string()))?;
        if let Some(error) = value.get("error").filter(|e| !e.is_null()) {
            let code = error["code"].as_i64().unwrap_or(0);
            let message = error["message"]
                .as_str()
                .unwrap_or("tool call failed")
                .to_owned();
            let code = if code == -32602 {
                Code::InvalidArgument
            } else {
                Code::Internal
            };
            return Err(status(code, ErrorReason::Unspecified, message));
        }
        Ok(Response::new(call_response(&value["result"])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::session::{is_session_ended, touch_session};

    #[tokio::test]
    async fn the_sweep_ends_a_session_left_idle_past_the_ttl() {
        let idle = format!("spacesd-sweep-idle-{}", std::process::id());
        let busy = format!("spacesd-sweep-busy-{}", std::process::id());
        touch_session(&idle);
        touch_session(&busy);
        let sweep = spawn_idle_sweep(Duration::from_millis(20), Duration::from_millis(300));
        // `busy` is used again and again; `idle` is left alone.
        for _ in 0..25 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            touch_session(&busy);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !is_session_ended(&idle) && std::time::Instant::now() < deadline {
            touch_session(&busy);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        sweep.abort();
        assert!(is_session_ended(&idle), "an idle session ends at its TTL");
        assert!(!is_session_ended(&busy), "a session in use is kept");
    }
}
