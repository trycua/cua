//! MCP to sandbox services with the official Rust SDK (`rmcp`).
//!
//! cua does not implement MCP. It gets bytes to the service
//! ([`Service::endpoint`]: a URL plus the headers the route needs) and hands
//! that to rmcp's streamable-HTTP client, which speaks every protocol
//! revision rmcp supports (`2026-07-28` via `server/discover`, falling back
//! to the `initialize` handshake of `2025-11-25` and older). Results are
//! rmcp's own model types, so every content block (text, image, audio,
//! resource links, embedded resources, annotations, `_meta`) arrives as the
//! server sent it. [`McpConfig`] is the same URL and headers for any other
//! MCP client (Claude Code, Cursor, the Python or TypeScript SDKs).

use crate::sandbox::{Sandbox, Service};
use crate::{Error, Result, ServiceEndpoint};
use rmcp::model::{ClientConfig, Implementation, ProtocolVersion};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RoleClient, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use std::collections::HashMap;

pub use rmcp;

/// A connected rmcp client (`client.peer()` has `list_all_tools`,
/// `call_tool`, `read_resource`, ...; `client.cancel()` closes it).
pub type McpClient = RunningService<RoleClient, ClientConfig>;

/// The default MCP endpoint path of a streamable-HTTP server.
pub const DEFAULT_PATH: &str = "/mcp";

/// Everything an MCP client needs to reach a service's MCP endpoint.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub struct McpConfig {
    /// The streamable-HTTP endpoint URL.
    pub url: String,
    /// Headers to send with every request (Fleet: the gateway bearer and
    /// claim; the bearer is short-lived, so fetch a fresh config per
    /// connection).
    pub headers: Vec<(String, String)>,
}

impl std::fmt::Debug for McpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpConfig")
            .field("url", &self.url)
            .field(
                "headers",
                &self.headers.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl McpConfig {
    /// The config for `path` (default `/mcp`) on `endpoint`.
    pub fn of(endpoint: &ServiceEndpoint, path: Option<&str>) -> Self {
        McpConfig {
            url: endpoint.url_for(path.filter(|p| !p.is_empty()).unwrap_or(DEFAULT_PATH)),
            headers: endpoint.headers.clone(),
        }
    }

    /// A config for a full endpoint URL (a bare origin means `/mcp`).
    pub fn for_url(url: &str, headers: Vec<(String, String)>) -> Result<Self> {
        let u = url::Url::parse(url)
            .map_err(|e| Error::InvalidArgument(format!("bad MCP URL {url:?}: {e}")))?;
        let url = if u.path().is_empty() || u.path() == "/" {
            format!("{}{DEFAULT_PATH}", u.origin().ascii_serialization())
        } else {
            url.to_string()
        };
        Ok(McpConfig { url, headers })
    }

    /// Connects rmcp's streamable-HTTP client (protocol `2026-07-28` when the
    /// server supports it, else the server's older revision).
    pub async fn connect(&self) -> Result<McpClient> {
        connect(self).await
    }
}

/// The protocol revisions cua asks for, newest first.
pub fn preferred_versions() -> Vec<ProtocolVersion> {
    vec![ProtocolVersion::V_2026_07_28, ProtocolVersion::V_2025_11_25]
}

/// Connects rmcp to `config`.
pub async fn connect(config: &McpConfig) -> Result<McpClient> {
    crate::http::ensure_tls();
    let mut headers = HashMap::new();
    for (k, v) in &config.headers {
        let name = http::HeaderName::try_from(k.as_str())
            .map_err(|e| Error::InvalidArgument(format!("header {k:?}: {e}")))?;
        let value = http::HeaderValue::try_from(v.as_str())
            .map_err(|e| Error::InvalidArgument(format!("header {k:?}: {e}")))?;
        headers.insert(name, value);
    }
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(config.url.as_str()).custom_headers(headers),
    );
    let info = ClientConfig::new(
        Default::default(),
        Implementation::new("cua", env!("CARGO_PKG_VERSION")),
    );
    info.serve_with_lifecycle(
        transport,
        ClientLifecycleMode::Auto {
            preferred_versions: preferred_versions(),
            legacy_version: None,
        },
    )
    .await
    .map_err(|e| Error::Mcp(format!("connect {}: {e}", config.url)))
}

impl Service {
    /// The MCP endpoint of this service at `path` (default `/mcp`): URL and
    /// headers, for any MCP client.
    pub async fn mcp_config(&self, path: Option<&str>) -> Result<McpConfig> {
        Ok(McpConfig::of(&self.endpoint().await?, path))
    }

    /// An rmcp client for this service's MCP endpoint at `path`.
    pub async fn mcp(&self, path: Option<&str>) -> Result<McpClient> {
        self.mcp_config(path).await?.connect().await
    }
}

impl Sandbox {
    /// [`Service::mcp_config`] of `service`.
    pub async fn mcp_config(&self, service: &str, path: Option<&str>) -> Result<McpConfig> {
        self.service(service)?.mcp_config(path).await
    }

    /// [`Service::mcp`] of `service`.
    pub async fn mcp(&self, service: &str, path: Option<&str>) -> Result<McpClient> {
        self.service(service)?.mcp(path).await
    }
}
