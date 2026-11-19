// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Typed cua-driver envelopes over `/mcp` (`ai.cua.driver.envelopes` v1).
//!
//! The wire contract is cua-driver's (`libs/cua-driver/docs/mcp-envelope-carrier.md`,
//! reference implementation `cua-driver/src/mcp_envelope.rs`): `initialize`
//! advertises `capabilities.experimental["ai.cua.driver.envelopes"] =
//! {"version": 1}`, and four acknowledged JSON-RPC methods carry the canonical
//! Driver envelopes:
//!
//! | Method                   | Parameters                                  |
//! | ------------------------ | ------------------------------------------- |
//! | `cua/driver/v1/open`     | `{}`                                        |
//! | `cua/driver/v1/exchange` | `connection_id`, `generation`, `envelope`   |
//! | `cua/driver/v1/cancel`   | `connection_id`, `generation`, `request_id` |
//! | `cua/driver/v1/close`    | `connection_id`, `generation`               |
//!
//! Malformed parameters answer `-32602`; receiver failures `-32000 - status`
//! (`-32404` foreign/missing connection, `-32409` stale generation); a full
//! exchange budget `-32029`.
//!
//! Generations, request ledgers, deadlines and cancellation are cua-driver's
//! own [`DriverEnvelopeReceiver`]. The executor behind it dispatches through
//! the same `cua_driver_core::server` JSON-RPC path as `tools/list` and
//! `tools/call` on this endpoint, with the same driver session (the MCP
//! session, or the agent's `X-Cua-Agent-Session`), so authorization is
//! exactly that of ordinary `/mcp` calls. No
//! listener, credential or tool is added: every request already passed the
//! spacesd bearer check, and one MCP session (the `mcp-session-id`) owns
//! its receivers; another session cannot address them. Deleting the MCP
//! session closes them.
//!
//! On by default whenever `/mcp` is served; `CUA_ENV_MCP_ENVELOPES=0` turns
//! the extension off (initialize then omits the capability and the methods
//! answer `-32601`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_driver_core::protocol::{Request as RpcRequest, Response as RpcResponse, ResponseBody};
use cua_driver_core::server::{handle_request_with_transport_session, ToolProvider};
use cua_driver_sdk::remote::DriverRequestEnvelope;
use cua_driver_sdk::remote_receiver::{DriverEnvelopeExecutor, DriverEnvelopeReceiver};
use cua_driver_sdk::{DriverError, DriverMetadata};
use serde::Deserialize;
use serde_json::{json, Value};

/// The MCP `experimental` capability name.
pub const CAPABILITY: &str = "ai.cua.driver.envelopes";
/// Method prefix of the typed control methods.
pub const PREFIX: &str = "cua/driver/v1/";
/// Environment switch: `0` disables the extension.
pub const ENV_SWITCH: &str = "CUA_ENV_MCP_ENVELOPES";

const MAX_CONNECTIONS: usize = 64;
const MAX_EXCHANGES: usize = 32;
const MAX_SESSIONS: usize = 256;
const MAX_RESPONSE: usize = 16 * 1024 * 1024;
const IDLE: Duration = Duration::from_secs(300);

/// Whether the extension is enabled (`CUA_ENV_MCP_ENVELOPES` is not `0`).
pub fn enabled() -> bool {
    std::env::var(ENV_SWITCH).map_or(true, |v| v.trim() != "0")
}

type RouteResult = Result<Value, (u16, &'static str)>;

struct Entry {
    receiver: Arc<DriverEnvelopeReceiver>,
    touched: Instant,
    active: usize,
}

struct SessionRegistry {
    entries: HashMap<String, Entry>,
    exchanges: Arc<tokio::sync::Semaphore>,
    touched: Instant,
}

impl SessionRegistry {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            exchanges: Arc::new(tokio::sync::Semaphore::new(MAX_EXCHANGES)),
            touched: Instant::now(),
        }
    }
}

/// Receivers of every MCP session on this server.
pub struct Envelopes {
    tools: Arc<dyn ToolProvider>,
    sessions: Mutex<HashMap<String, SessionRegistry>>,
}

/// One looked-up receiver; releases its activity count on drop.
struct Active {
    envelopes: Arc<Envelopes>,
    session: String,
    id: String,
    receiver: Arc<DriverEnvelopeReceiver>,
    exchanges: Arc<tokio::sync::Semaphore>,
}

impl Drop for Active {
    fn drop(&mut self) {
        let mut sessions = self.envelopes.sessions.lock().unwrap();
        if let Some(entry) = sessions
            .get_mut(&self.session)
            .and_then(|s| s.entries.get_mut(&self.id))
        {
            entry.active -= 1;
            entry.touched = Instant::now();
        }
    }
}

impl Envelopes {
    /// A registry dispatching through `tools`.
    pub fn new(tools: Arc<dyn ToolProvider>) -> Arc<Self> {
        Arc::new(Self {
            tools,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    /// Closes every receiver of `session` (the MCP session was deleted).
    pub fn close_session(&self, session: &str) {
        if let Some(registry) = self.sessions.lock().unwrap().remove(session) {
            for entry in registry.entries.values() {
                entry.receiver.close();
            }
        }
    }

    /// Drops idle receivers and empty sessions.
    fn reap(&self) {
        let mut sessions = self.sessions.lock().unwrap();
        for registry in sessions.values_mut() {
            registry.entries.retain(|_, entry| {
                if entry.active == 0 && entry.touched.elapsed() >= IDLE {
                    entry.receiver.close();
                    false
                } else {
                    true
                }
            });
        }
        sessions.retain(|_, r| !r.entries.is_empty() || r.touched.elapsed() < IDLE);
    }

    fn lookup(
        self: &Arc<Self>,
        session: &str,
        id: &str,
        generation: &str,
    ) -> Result<Active, (u16, &'static str)> {
        let mut sessions = self.sessions.lock().unwrap();
        let registry = sessions
            .get_mut(session)
            .ok_or((404, "connection_not_found"))?;
        let entry = registry
            .entries
            .get_mut(id)
            .ok_or((404, "connection_not_found"))?;
        if entry.receiver.generation() != generation {
            return Err((409, "stale_connection"));
        }
        entry.active += 1;
        entry.touched = Instant::now();
        registry.touched = Instant::now();
        Ok(Active {
            envelopes: self.clone(),
            session: session.to_owned(),
            id: id.to_owned(),
            receiver: entry.receiver.clone(),
            exchanges: registry.exchanges.clone(),
        })
    }

    fn open(self: &Arc<Self>, session: &str, driver: &str) -> RouteResult {
        let mut sessions = self.sessions.lock().unwrap();
        if !sessions.contains_key(session) && sessions.len() >= MAX_SESSIONS {
            return Err((503, "connection_limit"));
        }
        let registry = sessions
            .entry(session.to_owned())
            .or_insert_with(SessionRegistry::new);
        if registry.entries.len() >= MAX_CONNECTIONS {
            return Err((503, "connection_limit"));
        }
        let receiver = DriverEnvelopeReceiver::new(Arc::new(ToolExecutor {
            tools: self.tools.clone(),
            session: driver.to_owned(),
        }));
        let id = uuid::Uuid::new_v4().to_string();
        let response = json!({
            "connection_id": id,
            "generation": receiver.generation(),
            "capabilities": receiver.capabilities(),
            "public_session": session,
        });
        registry.entries.insert(
            id,
            Entry {
                receiver,
                touched: Instant::now(),
                active: 0,
            },
        );
        registry.touched = Instant::now();
        Ok(response)
    }

    /// Answers one typed control request of MCP session `session`; its
    /// receivers dispatch as cua-driver session `driver`.
    pub async fn handle(
        self: &Arc<Self>,
        request: &RpcRequest,
        session: &str,
        driver: &str,
    ) -> RpcResponse {
        let id = request.id.clone().unwrap_or(Value::Null);
        self.reap();
        let operation = match parse(request) {
            Ok(operation) => operation,
            Err(reason) => return RpcResponse::error(id, -32602, reason),
        };
        let result = match operation {
            Operation::Open => self.open(session, driver),
            Operation::Exchange(binding, envelope) => {
                match self.lookup(session, &binding.connection_id, &binding.generation) {
                    Err(e) => Err(e),
                    Ok(active) => {
                        let Ok(_permit) = active.exchanges.clone().try_acquire_owned() else {
                            return RpcResponse::error(id, -32029, "in_flight_limit");
                        };
                        // Decoded after lookup, as the receiver service does:
                        // a malformed envelope is its 400 `invalid_json`.
                        let envelope: DriverRequestEnvelope = match serde_json::from_value(envelope)
                        {
                            Ok(envelope) => envelope,
                            Err(_) => {
                                return RpcResponse::error(id, -32400, "invalid_json");
                            }
                        };
                        let mut response = active
                            .receiver
                            .exchange(&binding.generation, envelope)
                            .await;
                        if !response_fits(&response) {
                            active.receiver.close();
                            response.ok = false;
                            response.result = None;
                            response.error = Some(
                                "Driver response exceeded carrier limit; completion is unknown"
                                    .into(),
                            );
                            response.error_code = Some("response_too_large".into());
                            response.completion_known = false;
                        }
                        serde_json::to_value(response).map_err(|_| (500, "serialization_failed"))
                    }
                }
            }
            Operation::Cancel(binding, request_id) => {
                if request_id.is_empty() || request_id.len() > 256 {
                    Err((400, "invalid_request_id"))
                } else {
                    self.lookup(session, &binding.connection_id, &binding.generation)
                        .and_then(|active| {
                            active
                                .receiver
                                .cancel(&binding.generation, &request_id)
                                .map_err(|_| (409, "cancel_failed"))
                        })
                        .map(|()| json!({"ok": true}))
                }
            }
            Operation::Close(binding) => self
                .lookup(session, &binding.connection_id, &binding.generation)
                .map(|active| {
                    // The closed ledger stays until idle removal, so the old
                    // generation keeps being refused.
                    active.receiver.close();
                    json!({"ok": true})
                }),
        };
        match result {
            Ok(value) => RpcResponse::ok(id, value),
            Err((status, reason)) => RpcResponse::error(id, -32000 - i64::from(status), reason),
        }
    }
}

/// Adds the capability to a successful `initialize` result.
pub fn advertise(response: &mut RpcResponse) {
    if let ResponseBody::Result { result } = &mut response.body {
        if !result["capabilities"]["experimental"].is_object() {
            result["capabilities"]["experimental"] = json!({});
        }
        result["capabilities"]["experimental"][CAPABILITY] = json!({"version": 1});
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    connection_id: String,
    generation: String,
    #[serde(default)]
    envelope: Option<Value>,
    #[serde(default)]
    request_id: Option<String>,
}

enum Operation {
    Open,
    Exchange(Binding, Value),
    Cancel(Binding, String),
    Close(Binding),
}

fn parse(request: &RpcRequest) -> Result<Operation, &'static str> {
    let params = request.params.clone().ok_or("parameters_required")?;
    let operation = request.method.strip_prefix(PREFIX);
    if operation == Some("open") {
        if params != json!({}) {
            return Err("invalid_open_parameters");
        }
        return Ok(Operation::Open);
    }
    let fields = params.as_object().ok_or("invalid_binding")?;
    let allowed = match operation {
        Some("exchange") => &["connection_id", "generation", "envelope"][..],
        Some("cancel") => &["connection_id", "generation", "request_id"][..],
        Some("close") => &["connection_id", "generation"][..],
        _ => return Err("unknown_operation"),
    };
    if fields
        .keys()
        .any(|field| !allowed.contains(&field.as_str()))
    {
        return Err("invalid_operation_parameters");
    }
    let mut binding: Binding = serde_json::from_value(params).map_err(|_| "invalid_binding")?;
    if uuid::Uuid::parse_str(&binding.connection_id).is_err()
        || uuid::Uuid::parse_str(&binding.generation).is_err()
    {
        return Err("invalid_binding");
    }
    match operation {
        Some("exchange") => {
            if binding.request_id.is_some() {
                return Err("invalid_operation_parameters");
            }
            let envelope = binding.envelope.take().ok_or("envelope_required")?;
            Ok(Operation::Exchange(binding, envelope))
        }
        Some("cancel") if binding.envelope.is_none() => {
            let request_id = binding.request_id.take().ok_or("request_id_required")?;
            Ok(Operation::Cancel(binding, request_id))
        }
        Some("close") if binding.envelope.is_none() && binding.request_id.is_none() => {
            Ok(Operation::Close(binding))
        }
        _ => Err("invalid_operation_parameters"),
    }
}

fn response_fits(value: &impl serde::Serialize) -> bool {
    struct Budget(usize);
    impl std::io::Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(std::io::Error::other("response_too_large"));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Budget(MAX_RESPONSE), value).is_ok()
}

// The remote desktop subset cua-driver's receiver accepts (its private
// `remote_tool`): the inventory advertises only these.
fn remote_tool(name: &str) -> bool {
    matches!(
        name,
        "get_desktop_state"
            | "list_windows"
            | "get_window_state"
            | "get_screen_size"
            | "get_cursor_position"
            | "click"
            | "scroll"
            | "drag"
            | "move_cursor"
            | "type_text"
            | "press_key"
            | "hotkey"
            | "invoke_menu"
            | "set_window_frame"
            | "clipboard_read"
            | "clipboard_write"
            | "verify_state"
            | "get_agent_cursor_state"
            | "set_agent_cursor_enabled"
            | "set_agent_cursor_motion"
            | "set_agent_cursor_theme"
    )
}

/// Dispatches receiver operations through this endpoint's MCP handler.
struct ToolExecutor {
    tools: Arc<dyn ToolProvider>,
    session: String,
}

impl ToolExecutor {
    async fn rpc(&self, method: &str, params: Option<Value>) -> Result<Value, (i64, String)> {
        let request: RpcRequest = serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        }))
        .map_err(|e| (-32603, e.to_string()))?;
        let response = handle_request_with_transport_session(
            request,
            json!(1),
            self.tools.as_ref(),
            &self.session,
        )
        .await;
        match response.body {
            ResponseBody::Result { result } => Ok(result),
            ResponseBody::Error { error } => {
                let value = serde_json::to_value(&error).unwrap_or(Value::Null);
                Err((
                    value.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("tool call failed")
                        .to_owned(),
                ))
            }
        }
    }
}

fn protocol_error(reason: impl Into<String>) -> DriverError {
    DriverError::Protocol {
        reason: reason.into(),
    }
}

#[async_trait::async_trait]
impl DriverEnvelopeExecutor for ToolExecutor {
    async fn metadata(&self) -> Result<Value, DriverError> {
        let driver_version = cua_driver_core::protocol::initialize_result()["serverInfo"]
            ["version"]
            .as_str()
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .to_owned();
        serde_json::to_value(DriverMetadata {
            driver_version,
            contract_version: cua_driver_contract::CONTRACT_VERSION.into(),
            tools_list_schema_version: cua_driver_contract::TOOLS_LIST_SCHEMA_VERSION.into(),
            capability_version: cua_driver_contract::CAPABILITY_VERSION.into(),
            mcp_protocol_version: cua_driver_contract::MCP_PROTOCOL_VERSION.into(),
            pid: std::process::id(),
            embedded: false,
            host_bundle_id: None,
        })
        .map_err(|e| protocol_error(e.to_string()))
    }

    async fn list_tools(&self) -> Result<Value, DriverError> {
        let mut inventory = self
            .rpc("tools/list", None)
            .await
            .map_err(|(_, message)| protocol_error(message))?;
        if let Some(tools) = inventory.get_mut("tools").and_then(Value::as_array_mut) {
            tools.retain(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(remote_tool)
            });
        }
        Ok(inventory)
    }

    async fn call(&self, name: String, arguments: Value) -> Result<Value, DriverError> {
        // A tool-level failure is an `isError` result, returned as-is (the
        // SDK's `raw_json`); only a call that could not dispatch errors here.
        self.rpc(
            "tools/call",
            Some(json!({"name": name, "arguments": arguments})),
        )
        .await
        .map_err(|(_, message)| DriverError::Tool {
            tool: name,
            message,
            error_code: "tool_unavailable".into(),
        })
    }

    fn close(&self) {}
}
