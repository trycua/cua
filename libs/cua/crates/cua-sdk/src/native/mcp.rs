//! MCP to sandbox services: `Sandbox.mcp_config(service)` (URL + headers for
//! any MCP client) and `Sandbox.mcp(service)`, a client built on the official
//! Rust SDK (rmcp). cua does not re-model MCP: every method returns the
//! server's result as JSON, so content blocks (text, image, audio, resource
//! links, embedded resources, annotations, `_meta`) are lossless by
//! construction. In daemon mode the URL is the daemon's streaming
//! passthrough (`/v1/sandboxes/<name>/svc/<service>/`).

use super::{Backend, HttpHeader, run};
use crate::{CuaError, Result};
use cua_sandbox_core::mcp::rmcp::model::{
    CallToolRequestParams, ClientRequest, CustomRequest, GetPromptRequestParams,
    ReadResourceRequestParams,
};
use cua_sandbox_core::{McpConfig as CoreConfig, ServiceEndpoint};
use serde_json::{Map, Value};
use std::sync::Arc;

/// Where an MCP endpoint is and what every request needs. Hand it to any
/// MCP client (Claude Code, Cursor, the Python or TypeScript SDKs). On
/// Fleet the headers carry a short-lived gateway bearer: fetch a fresh
/// config per connection.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct McpConfig {
    /// Streamable-HTTP endpoint URL.
    pub url: String,
    /// Headers to send with every request.
    pub headers: Vec<HttpHeader>,
}

impl From<CoreConfig> for McpConfig {
    fn from(c: CoreConfig) -> Self {
        McpConfig {
            url: c.url,
            headers: c
                .headers
                .into_iter()
                .map(|(name, value)| HttpHeader { name, value })
                .collect(),
        }
    }
}

impl From<&McpConfig> for CoreConfig {
    fn from(c: &McpConfig) -> Self {
        CoreConfig {
            url: c.url.clone(),
            headers: c
                .headers
                .iter()
                .map(|h| (h.name.clone(), h.value.clone()))
                .collect(),
        }
    }
}

fn mcp_err(e: impl std::fmt::Display) -> CuaError {
    CuaError::Http(format!("mcp: {e}"))
}

fn json<T: serde::Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|e| CuaError::Internal(e.to_string()))
}

fn object(arguments_json: Option<String>) -> Result<Map<String, Value>> {
    match arguments_json.as_deref().map(str::trim) {
        None | Some("") => Ok(Map::new()),
        Some(j) => match serde_json::from_str::<Value>(j)
            .map_err(|e| CuaError::InvalidArgument(format!("bad JSON: {e}")))?
        {
            Value::Object(m) => Ok(m),
            _ => Err(CuaError::InvalidArgument(
                "arguments must be a JSON object".into(),
            )),
        },
    }
}

/// The endpoint of `service` of sandbox `name` for this backend.
pub(crate) async fn service_endpoint(
    backend: &Backend,
    name: &str,
    service: &str,
) -> Result<ServiceEndpoint> {
    Ok(match backend {
        Backend::Embedded(rt) => rt.service_endpoint(name, service).await?,
        Backend::Daemon(d) => d.service_endpoint(name, service).await?,
    })
}

/// An MCP client (rmcp, streamable HTTP; protocol 2026-07-28 when the
/// server supports it, else the server's revision). Results are the
/// server's JSON, unchanged.
#[derive(uniffi::Object)]
pub struct McpClient {
    inner: Arc<cua_sandbox_core::McpClient>,
}

impl McpClient {
    pub(crate) async fn connect(config: CoreConfig) -> Result<Arc<Self>> {
        let inner = config.connect().await?;
        Ok(Arc::new(Self {
            inner: Arc::new(inner),
        }))
    }
}

/// Connects an MCP client to an endpoint URL (`http://host:8765/mcp`; a
/// bare origin means `/mcp`) with `headers` on every request.
#[uniffi::export]
pub async fn mcp_connect_url(
    url: String,
    headers: Option<Vec<HttpHeader>>,
) -> Result<Arc<McpClient>> {
    let headers = headers
        .unwrap_or_default()
        .into_iter()
        .map(|h| (h.name, h.value))
        .collect();
    run(async move { McpClient::connect(CoreConfig::for_url(&url, headers)?).await }).await
}

/// Connects an MCP client to `config` (from `Sandbox.mcp_config`).
#[uniffi::export]
pub async fn mcp_connect(config: McpConfig) -> Result<Arc<McpClient>> {
    let c = CoreConfig::from(&config);
    run(async move { McpClient::connect(c).await }).await
}

#[uniffi::export]
impl McpClient {
    /// The server's `serverInfo`/negotiated version as JSON, when known.
    pub fn server_info_json(&self) -> Option<String> {
        self.inner.peer_info().and_then(|i| json(&*i).ok())
    }

    /// Every tool (all pages) as a JSON array of MCP `Tool` objects.
    pub async fn list_tools(&self) -> Result<String> {
        let c = self.inner.clone();
        run(async move { json(&c.peer().list_all_tools().await.map_err(mcp_err)?) }).await
    }

    /// Calls `name` with a JSON object of arguments (`None`: `{}`); returns
    /// the `tools/call` result JSON verbatim (a tool error is `isError`).
    pub async fn call_tool(&self, name: String, arguments_json: Option<String>) -> Result<String> {
        let args = object(arguments_json)?;
        let c = self.inner.clone();
        run(async move {
            let r = c
                .peer()
                .call_tool(CallToolRequestParams::new(name).with_arguments(args))
                .await
                .map_err(mcp_err)?;
            json(&r)
        })
        .await
    }

    /// Every resource (all pages), JSON array.
    pub async fn list_resources(&self) -> Result<String> {
        let c = self.inner.clone();
        run(async move { json(&c.peer().list_all_resources().await.map_err(mcp_err)?) }).await
    }

    /// Every resource template (all pages), JSON array.
    pub async fn list_resource_templates(&self) -> Result<String> {
        let c = self.inner.clone();
        run(async move {
            json(
                &c.peer()
                    .list_all_resource_templates()
                    .await
                    .map_err(mcp_err)?,
            )
        })
        .await
    }

    /// Reads `uri`; the `resources/read` result JSON.
    pub async fn read_resource(&self, uri: String) -> Result<String> {
        let c = self.inner.clone();
        run(async move {
            json(
                &c.peer()
                    .read_resource(ReadResourceRequestParams::new(uri))
                    .await
                    .map_err(mcp_err)?,
            )
        })
        .await
    }

    /// Every prompt (all pages), JSON array.
    pub async fn list_prompts(&self) -> Result<String> {
        let c = self.inner.clone();
        run(async move { json(&c.peer().list_all_prompts().await.map_err(mcp_err)?) }).await
    }

    /// Renders prompt `name`; the `prompts/get` result JSON.
    pub async fn get_prompt(&self, name: String, arguments_json: Option<String>) -> Result<String> {
        let args = object(arguments_json)?;
        let c = self.inner.clone();
        run(async move {
            let mut p = GetPromptRequestParams::new(name);
            if !args.is_empty() {
                p = p.with_arguments(args);
            }
            json(&c.peer().get_prompt(p).await.map_err(mcp_err)?)
        })
        .await
    }

    /// Any other JSON-RPC method (extensions such as `skills/list`,
    /// `skills/get`); returns its `result` JSON.
    pub async fn request(&self, method: String, params_json: Option<String>) -> Result<String> {
        let params = match params_json.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(j) => Some(
                serde_json::from_str::<Value>(j)
                    .map_err(|e| CuaError::InvalidArgument(format!("bad JSON: {e}")))?,
            ),
        };
        let c = self.inner.clone();
        run(async move {
            let r = c
                .peer()
                .send_request(ClientRequest::CustomRequest(CustomRequest::new(
                    method, params,
                )))
                .await
                .map_err(mcp_err)?;
            json(&r)
        })
        .await
    }

    /// Closes the connection (ends the session on servers that keep one).
    pub async fn close(&self) -> Result<()> {
        let c = self.inner.clone();
        run(async move {
            c.cancellation_token().cancel();
            Ok(())
        })
        .await
    }
}
