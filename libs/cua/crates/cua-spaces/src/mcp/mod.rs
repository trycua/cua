//! The Spaces MCP server: the contract's tools over [`crate::Spaces`].
//!
//! `cua daemon mcp` hosts this. [`McpServer::handle`] is transport-free
//! JSON-RPC; [`stdio::serve`] frames it as newline-delimited JSON on
//! stdin/stdout, and [`http::router`] serves MCP streamable HTTP (`POST
//! /mcp`, JSON responses, `Mcp-Session-Id`).
//!
//! `tools/list` is built from `cua_spaces_contract::tools()` and every
//! `tools/call` argument object is deserialized into the contract's
//! `inputs` type the published schema was derived from, so the schema and
//! the parser cannot drift. Before dispatch, each tool's `requires`
//! features are checked against the Space's spacesd capabilities, and a
//! missing one is a `capability_missing` tool error naming it.

/// The Cua Volume tools, runnable as any caller context (the MCP server as
/// the user, the in-Space agent bridge as the run's agent).
mod tools;

pub mod gate;
pub mod stdio;
pub mod surface;

#[cfg(feature = "mcp-http")]
pub mod http;

use crate::{Space, Spaces};
use serde_json::{Value, json};
use std::sync::Arc;

/// Server name in `initialize`.
pub const SERVER_NAME: &str = "cua-spaces";
/// Server version in `initialize`.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Protocol revisions this server accepts, newest first.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// JSON-RPC error codes.
pub mod codes {
    /// The message is not valid JSON.
    pub const PARSE_ERROR: i64 = -32700;
    /// The message is not a JSON-RPC request: an empty batch, or no `method`.
    pub const INVALID_REQUEST: i64 = -32600;
    /// An unknown method, or `tools/call` of a tool this server does not
    /// expose (not in the contract, or filtered out by permissions).
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// `tools/call` without a tool `name`.
    pub const INVALID_PARAMS: i64 = -32602;
}

/// The result of one tool call, as MCP content.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutcome {
    /// Content parts.
    pub content: Vec<Value>,
    /// Structured content, when there is one.
    pub structured: Option<Value>,
    /// The call failed (as a tool error, not a protocol error).
    pub is_error: bool,
    /// `_meta` of a forwarded result, verbatim.
    pub meta: Option<Value>,
}

impl ToolOutcome {
    /// A text result.
    pub fn text(text: impl Into<String>) -> Self {
        ToolOutcome {
            content: vec![json!({"type": "text", "text": text.into()})],
            structured: None,
            is_error: false,
            meta: None,
        }
    }

    /// A JSON result (pretty-printed text content).
    pub fn json<T: serde::Serialize>(value: &T) -> Self {
        let v = serde_json::to_value(value).unwrap_or(Value::Null);
        ToolOutcome {
            content: vec![json!({
                "type": "text",
                "text": serde_json::to_string_pretty(&v).unwrap_or_default(),
            })],
            structured: None,
            is_error: false,
            meta: None,
        }
    }

    /// A tool error.
    pub fn error(e: &crate::Error) -> Self {
        ToolOutcome {
            content: vec![json!({"type": "text", "text": format!("error: {e}")})],
            structured: Some(json!({"error": {"kind": e.tag(), "message": e.to_string()}})),
            is_error: true,
            meta: None,
        }
    }

    /// A tool error from any message, tagged `kind`.
    pub fn error_message(kind: &str, message: impl Into<String>) -> Self {
        let message = message.into();
        ToolOutcome {
            content: vec![json!({"type": "text", "text": format!("error: {message}")})],
            structured: Some(json!({"error": {"kind": kind, "message": message}})),
            is_error: true,
            meta: None,
        }
    }

    /// The text of the first text content part.
    pub fn first_text(&self) -> Option<&str> {
        self.content
            .iter()
            .find_map(|c| c.get("text").and_then(Value::as_str))
    }

    /// The MCP `tools/call` result object.
    pub fn to_result(&self) -> Value {
        let mut r = json!({"content": self.content, "isError": self.is_error});
        if let Some(s) = &self.structured {
            r["structuredContent"] = s.clone();
        }
        if let Some(m) = &self.meta {
            r["_meta"] = m.clone();
        }
        r
    }
}

/// Where contract tool calls go when the server does not own a
/// [`Spaces`]: `cua daemon mcp` and `cua mcp` forward them to the daemon's
/// server (`SpaceService.CallTool`), so there is one implementation of every
/// tool and one registry.
#[async_trait::async_trait]
pub trait ToolBackend: Send + Sync {
    /// Calls one contract tool.
    async fn call(&self, tool: &str, arguments: Value) -> ToolOutcome;
}

/// Tools served next to the Spaces contract (the `cua` CLI's sandbox,
/// computer and skills tools). Names must not collide with contract tools.
#[async_trait::async_trait]
pub trait ToolExtension: Send + Sync {
    /// `tools/list` entries (`name`, `description`, `inputSchema`, ...).
    fn tools(&self) -> Vec<Value>;
    /// Calls `tool` if it is one of [`ToolExtension::tools`]; `None` otherwise.
    async fn call(&self, tool: &str, arguments: Value) -> Option<ToolOutcome>;
}

#[derive(Clone)]
enum Backend {
    Local(Spaces),
    Remote(Arc<dyn ToolBackend>),
}

/// Which contract tools a server exposes (for example `cua mcp
/// --permissions`). Returns true to expose.
pub type ToolFilter = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// The server. Cheap to clone.
#[derive(Clone)]
pub struct McpServer {
    backend: Backend,
    extensions: Vec<Arc<dyn ToolExtension>>,
    filter: Option<ToolFilter>,
    name: &'static str,
    /// `instructions` in `initialize` (default [`INSTRUCTIONS`]).
    instructions: &'static str,
    /// The Keyvault broker `teleport_app` routes delivery through. `None`
    /// keeps the tool fail-closed (it returns a consent requirement and
    /// delivers nothing).
    broker: Option<Arc<dyn crate::teleport_broker::SessionBroker>>,
    /// The agent surface (see [`surface`]): the small hand-written tool list,
    /// the approval gate and the identity the Volume sees. `None` serves the
    /// full contract (daemon, SDK and in-Space agents).
    agent: Option<AgentSurface>,
}

#[derive(Clone)]
struct AgentSurface {
    guard: gate::Guard,
    /// The calling agent's name (from `clientInfo`): the Volume principal.
    client: Arc<std::sync::Mutex<String>>,
    /// The Space tools that name none act on (`cua mcp --sandbox`).
    default_space: String,
}

impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServer")
            .field("name", &self.name)
            .field("local", &matches!(self.backend, Backend::Local(_)))
            .field("extensions", &self.extensions.len())
            .finish()
    }
}

impl McpServer {
    /// A server over `spaces`.
    pub fn new(spaces: Spaces) -> Self {
        Self {
            backend: Backend::Local(spaces),
            extensions: vec![],
            filter: None,
            name: SERVER_NAME,
            instructions: INSTRUCTIONS,
            broker: None,
            agent: None,
        }
    }

    /// A server whose contract tools run elsewhere (a daemon).
    pub fn remote(backend: Arc<dyn ToolBackend>) -> Self {
        Self {
            backend: Backend::Remote(backend),
            extensions: vec![],
            filter: None,
            name: SERVER_NAME,
            instructions: INSTRUCTIONS,
            broker: None,
            agent: None,
        }
    }

    /// Routes `teleport_app` delivery through the Keyvault broker. Without
    /// one the tool stays fail-closed. The broker treats the MCP caller as an
    /// unverified third party, so a delivery needs a live consent (Touch ID)
    /// or a matching unattended rule (design 5.12).
    pub fn with_session_broker(
        mut self,
        broker: Arc<dyn crate::teleport_broker::SessionBroker>,
    ) -> Self {
        self.broker = Some(broker);
        self
    }

    /// Serves the agent surface: a small tool list ([`surface`]), every call
    /// checked by `guard` (the user's approval policy), and Volume calls made
    /// as the calling agent rather than as the user.
    pub fn with_agent_surface(mut self, guard: gate::Guard) -> Self {
        self.agent = Some(AgentSurface {
            guard,
            client: Arc::new(std::sync::Mutex::new("agent".to_string())),
            default_space: String::new(),
        });
        self.instructions = surface::INSTRUCTIONS;
        self
    }

    /// The Space an agent-surface call acts on when it names none.
    pub fn with_default_space(mut self, space: impl Into<String>) -> Self {
        if let Some(a) = self.agent.as_mut() {
            a.default_space = space.into();
        }
        self
    }

    /// Adds non-contract tools.
    pub fn with_extension(mut self, extension: Arc<dyn ToolExtension>) -> Self {
        self.extensions.push(extension);
        self
    }

    /// Exposes only the contract tools `filter` accepts.
    pub fn with_filter(mut self, filter: ToolFilter) -> Self {
        self.filter = Some(filter);
        self
    }

    /// `serverInfo.name` in `initialize` (default `cua-spaces`).
    pub fn with_name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// `instructions` in `initialize` (default [`INSTRUCTIONS`]).
    pub fn with_instructions(mut self, instructions: &'static str) -> Self {
        self.instructions = instructions;
        self
    }

    /// The Spaces runtime, when this server owns one.
    pub fn spaces(&self) -> Option<&Spaces> {
        match &self.backend {
            Backend::Local(s) => Some(s),
            Backend::Remote(_) => None,
        }
    }

    fn exposes(&self, tool: &str) -> bool {
        let tool = cua_spaces_contract::canonical(tool);
        cua_spaces_contract::tool(tool).is_some() && self.filter.as_ref().is_none_or(|f| f(tool))
    }

    /// `tools/list` result: the contract tools, then the extensions'.
    pub fn tools_list(&self) -> Value {
        if self.agent.is_some() {
            let tools: Vec<Value> = surface::tools()
                .into_iter()
                .filter(|t| {
                    let name = t["name"].as_str().unwrap_or_default();
                    matches!(name, "more" | "approvals")
                        || surface::underlying(name).iter().any(|u| self.knows(u))
                })
                .collect();
            return json!({ "tools": tools });
        }
        let mut tools: Vec<Value> = cua_spaces_contract::tools()
            .into_iter()
            .filter(|t| self.exposes(t.name))
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.mcp_description(),
                    "inputSchema": t.input_schema,
                    "annotations": {
                        "readOnlyHint": t.annotations.read_only,
                        "destructiveHint": t.annotations.destructive,
                        "idempotentHint": t.annotations.idempotent,
                        "openWorldHint": t.annotations.open_world,
                    },
                })
            })
            .collect();
        for e in &self.extensions {
            tools.extend(e.tools());
        }
        json!({ "tools": tools })
    }

    /// Calls one contract tool (no filter applied).
    pub async fn call(&self, name: &str, arguments: Value) -> ToolOutcome {
        let name = cua_spaces_contract::canonical(name);
        match &self.backend {
            Backend::Local(spaces) => {
                tools::call(spaces, name, arguments, self.broker.as_ref()).await
            }
            Backend::Remote(b) => b.call(name, arguments).await,
        }
    }

    fn knows(&self, name: &str) -> bool {
        self.exposes(name)
            || self.extensions.iter().any(|e| {
                e.tools()
                    .iter()
                    .any(|t| t.get("name").and_then(Value::as_str) == Some(name))
            })
    }

    async fn call_any(&self, name: &str, arguments: Value) -> ToolOutcome {
        if self.exposes(name) {
            return self.call(name, arguments).await;
        }
        for e in &self.extensions {
            if let Some(out) = e.call(name, arguments.clone()).await {
                return out;
            }
        }
        ToolOutcome::error(&crate::Error::NotFound(format!("tool {name}")))
    }

    /// Whether `space` (an id or a name) is one of the user's own machines.
    /// Fails closed when the Spaces cannot be listed.
    async fn is_machine(&self, space: &str) -> bool {
        if space.is_empty() || space.starts_with("local:") || space.starts_with("cloud:") {
            return false;
        }
        if space.starts_with("relay:") || space.starts_with("direct:") {
            return true;
        }
        let out = self.call("list_spaces", json!({})).await;
        let Some(rows) = out
            .first_text()
            .filter(|_| !out.is_error)
            .and_then(|t| serde_json::from_str::<Vec<Value>>(t).ok())
        else {
            return true;
        };
        rows.iter()
            .find(|r| r["id"] == space || r["name"] == space)
            .and_then(|r| r["id"].as_str())
            .is_some_and(|id| id.starts_with("relay:") || id.starts_with("direct:"))
    }

    /// The id of the Space called `name` (ids are returned as they are).
    async fn space_id(&self, name: &str) -> Option<String> {
        let out = self.call("list_spaces", json!({})).await;
        let rows = out
            .first_text()
            .filter(|_| !out.is_error)
            .and_then(|t| serde_json::from_str::<Vec<Value>>(t).ok())?;
        rows.iter()
            .find(|r| r["name"] == name)
            .and_then(|r| r["id"].as_str().map(str::to_string))
    }

    /// One `tools/call` on the agent surface: routes a facade tool to the tool
    /// it stands for, applies the approval gate, then runs it.
    async fn invoke_agent(&self, agent: &AgentSurface, name: &str, args: Value) -> ToolOutcome {
        let client = agent.client.lock().unwrap().clone();
        match name {
            "approvals" => {
                let p = agent.guard.policy();
                return ToolOutcome::json(&json!({
                    "ask_for_approval": p.rows().iter().map(|r| json!({"id": r.id, "title": r.title, "needs_approval": r.require})).collect::<Vec<_>>(),
                    "always_asks": crate::approvals::always().iter().map(|a| a.title).collect::<Vec<_>>(),
                }));
            }
            "more" => return self.more(agent, args).await,
            _ => {}
        }
        let (tool, mut args) = match surface::route(name, &args) {
            Some(Ok(routed)) => routed,
            Some(Err(e)) => return ToolOutcome::error(&e),
            None => (name.to_string(), args),
        };
        let tool = cua_spaces_contract::canonical(&tool).to_string();
        if !self.knows(&tool) {
            return ToolOutcome::error(&crate::Error::NotFound(format!("tool {name}")));
        }
        // The tools that take a sandbox ref get the id of the Space an agent
        // names by its display name.
        if let Some(key) = gate::sandbox_key(&tool)
            && let Some(named) = args[key]
                .as_str()
                .filter(|n| !n.is_empty() && !n.contains(':'))
            && let Some(id) = self.space_id(named).await
        {
            args[key] = json!(id);
        }
        for need in gate::classify(&tool, &args) {
            let (cap, what) = match need {
                gate::Need::Forbidden(why) => return ToolOutcome::error_message("forbidden", why),
                gate::Need::Cap(cap, what) => (cap, what),
                gate::Need::OnMachine { space, what } => {
                    let space = if space.is_empty() {
                        agent.default_space.clone()
                    } else {
                        space
                    };
                    if !self.is_machine(&space).await {
                        continue;
                    }
                    (crate::approvals::Cap::RemoteExec, what)
                }
            };
            if let Err(why) = agent.guard.require(&client, cap, &what).await {
                return ToolOutcome::error_message(
                    "approval_denied",
                    format!(
                        "{why}. \"{}\" needs your approval (Cua Settings > Agent approvals). Tell the user; do not retry another way.",
                        cap.title()
                    ),
                );
            }
        }
        if surface::is_volume_data_tool(&tool)
            && let Some(o) = args.as_object_mut()
        {
            // The calling agent sees its own home and `public/`, never the
            // whole volume as the user, and cannot act as another agent.
            o.insert("as_agent".into(), json!(client));
            o.remove("in_space");
        }
        self.call_any(&tool, args).await
    }

    /// `more`: list the rarely used tools, describe one, or call one.
    async fn more(&self, agent: &AgentSurface, args: Value) -> ToolOutcome {
        let name = args["name"].as_str().unwrap_or_default();
        if name.is_empty() {
            let mut lines = vec![];
            for t in surface::ADVANCED {
                if !self.knows(t) {
                    continue;
                }
                lines.push(format!("{t}: {}", self.summary_of(t)));
            }
            return ToolOutcome::text(lines.join("\n"));
        }
        if !surface::ADVANCED.contains(&name) || !self.knows(name) {
            return ToolOutcome::error(&crate::Error::NotFound(format!(
                "more tool {name}; call more with no name to list them"
            )));
        }
        match args.get("arguments").filter(|a| !a.is_null()) {
            Some(call) => {
                // The same checks as any call: the name is only a route.
                let args = call.clone();
                Box::pin(self.invoke_agent(agent, name, args)).await
            }
            None => ToolOutcome::json(&self.describe(name)),
        }
    }

    fn describe(&self, name: &str) -> Value {
        if let Some(t) = cua_spaces_contract::tool(name) {
            return json!({"name": t.name, "description": t.mcp_description(), "inputSchema": t.input_schema});
        }
        for e in &self.extensions {
            if let Some(t) = e
                .tools()
                .into_iter()
                .find(|t| t["name"].as_str() == Some(name))
            {
                return t;
            }
        }
        json!({"name": name})
    }

    /// The first sentence of a tool's description.
    fn summary_of(&self, name: &str) -> String {
        let d = self.describe(name);
        let text = d["description"].as_str().unwrap_or_default();
        let text = text.split("\n\n").next().unwrap_or(text);
        match text.find(". ") {
            Some(i) => text[..=i].to_string(),
            None => text.to_string(),
        }
    }

    /// Handles one JSON-RPC message (or batch). Returns the response, or
    /// `None` for a notification.
    pub async fn handle(&self, message: Value) -> Option<Value> {
        if let Value::Array(batch) = message {
            if batch.is_empty() {
                return Some(error(Value::Null, codes::INVALID_REQUEST, "empty batch"));
            }
            let mut out = vec![];
            for m in batch {
                if let Some(r) = Box::pin(self.handle(m)).await {
                    out.push(r);
                }
            }
            return (!out.is_empty()).then_some(Value::Array(out));
        }
        let Some(obj) = message.as_object() else {
            return Some(error(
                Value::Null,
                codes::INVALID_REQUEST,
                "not a JSON-RPC object",
            ));
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // A response from the client (we send no requests): ignore.
            return id
                .filter(|_| !obj.contains_key("result") && !obj.contains_key("error"))
                .map(|id| error(id, codes::INVALID_REQUEST, "missing method"));
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        let id = id?; // notifications get no response
        let result = match method {
            "initialize" => {
                if let Some(agent) = &self.agent {
                    let client = params["clientInfo"]["name"].as_str().unwrap_or("agent");
                    *agent.client.lock().unwrap() = surface::client_identity(client);
                }
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&asked) {
                    asked
                } else {
                    SUPPORTED_PROTOCOL_VERSIONS[0]
                };
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": self.name, "version": SERVER_VERSION},
                    "instructions": self.instructions,
                })
            }
            "ping" => json!({}),
            "tools/list" => self.tools_list(),
            "tools/call" => {
                let Some(name) = params.get("name").and_then(Value::as_str) else {
                    return Some(error(id, codes::INVALID_PARAMS, "tools/call needs a name"));
                };
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                if let Some(agent) = &self.agent {
                    if !surface::is_facade(name) && !self.knows(name) {
                        return Some(error(
                            id,
                            codes::METHOD_NOT_FOUND,
                            &format!("unknown tool {name}"),
                        ));
                    }
                    return Some(
                        json!({"jsonrpc": "2.0", "id": id, "result": self.invoke_agent(agent, name, args).await.to_result()}),
                    );
                }
                if !self.knows(name) {
                    return Some(error(
                        id,
                        codes::METHOD_NOT_FOUND,
                        &format!("unknown tool {name}"),
                    ));
                }
                self.call_any(name, args).await.to_result()
            }
            other => {
                return Some(error(
                    id,
                    codes::METHOD_NOT_FOUND,
                    &format!("unknown method {other}"),
                ));
            }
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }
}

/// Runs a Space-scoped contract tool against an already-connected
/// [`Space`] instead of a registered one (the `space` argument is ignored).
/// The `cua` CLI serves its `computer_shell` / `computer_file_write` through
/// this, so they share the `space_bash` / `space_write` implementation.
/// Only tools that need nothing but the Space are accepted.
pub async fn call_on(space: &Space, tool: &str, arguments: Value) -> ToolOutcome {
    tools::call_on(space, tool, arguments).await
}

/// Tools [`call_on`] accepts.
pub const SPACE_SCOPED_TOOLS: &[&str] = &[
    "space_bash",
    "space_write",
    "upload",
    "download",
    "send_file",
    "list_tools",
    "call_tool",
    "list_space_windows",
    "stream_endpoint",
];

/// Server instructions sent in `initialize`.
pub const INSTRUCTIONS: &str = "Cua Spaces: isolated sandboxes. Any image is a Space; the computer-use and file tools need one that runs cua-spacesd (the Spaces images), and every Space reaches the MCP servers it declares with list_tools/call_tool(service=...). Start with list_spaces; create one with create_space(on=\"local\" (free) or \"cloud\" (metered); reuse=true returns a reachable one), or add an existing machine with add_space(url, token). delete_space deletes a Space you created. Then work inside it with space_bash, space_write, upload/send_file/download, and its computer-use tools via list_tools/call_tool. agent_start runs a coding agent inside a Space. Tools marked FOR THE HUMAN draw on the user's own desktop; call them only when asked.";

pub(crate) fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
