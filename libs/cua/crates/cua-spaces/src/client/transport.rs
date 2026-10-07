//! The MCP stdio transport, and the one seam where a failure is turned from a
//! value into an error.
//!
//! ## `FRICTION.md` §3, and why it cannot happen here
//!
//! The Spaces server does **not** report a failing tool as a JSON-RPC error.
//! It returns a normal, successful result whose `isError` is `true` and whose
//! one text part reads `error: …`. The obvious client therefore reads an error
//! message where it expects a return value, and nothing anywhere errors.
//!
//! The structural fix is that **no type in this crate can hold both a payload
//! and a failure flag**. `decode_tool_result` is the only function that ever
//! looks at `isError`, it is the only way a `tools/call` result becomes a
//! value, and its return type is `Result<Value, SpacesError>` — a sum, so a
//! caller cannot hold the payload without having discharged the failure. There
//! is no `ToolOutcome { is_error, text }` to read the wrong field of, because
//! that struct is exactly the bug.
//!
//! The trait callers program against, `ToolTransport`, has the same shape, so
//! the guarantee survives into every fake backend as well as the live one.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{Map, Value, json};

use crate::client::error::{Result, SpacesError};

/// The MCP revision the client announces.
pub const CLIENT_PROTOCOL_VERSION: &str = "2024-11-05";
pub const CLIENT_NAME: &str = "cua-spaces-client";
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The one thing the control plane needs from a Spaces backend: call a named
/// tool with named arguments and get JSON back, **or** fail.
///
/// Everything above this line is written against this trait and nothing else,
/// so the conformance suite drives the whole surface against an in-process
/// fake and a live session drives the same code against a real Space.
pub trait ToolTransport: Send + Sync {
    /// The tool's payload, already parsed when it was JSON and a JSON string
    /// when the tool answered in prose.
    ///
    /// Implementors must map any `isError` result to `Err`, which
    /// `decode_tool_result` does for them.
    fn call_tool(&self, name: &str, arguments: &Value) -> Result<Value>;

    /// Tool names the backend offers. Empty when the transport cannot say.
    fn available_tools(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

/// The **only** place `isError` is read, and the only way a `tools/call`
/// result becomes a value.
///
/// `result` is the raw MCP `result` object. Returns the payload, or an error
/// carrying whatever the server said. A payload and a failure are never both
/// representable: the return type is a sum.
pub fn decode_tool_result(tool: &str, result: &Value) -> Result<Value> {
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let text = content
        .first()
        .and_then(|part| part.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(SpacesError::ToolFailed {
            tool: tool.to_string(),
            message: text,
        });
    }
    if text.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    Ok(serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)))
}

/// The payload as an object, or a malformed-response error.
pub fn expect_object(tool: &str, payload: &Value) -> Result<Map<String, Value>> {
    payload
        .as_object()
        .cloned()
        .ok_or_else(|| SpacesError::malformed(tool, format!("not an object: {payload}")))
}

/// The payload as an array, tolerating the tools that answer "none" with an
/// English sentence.
///
/// `FRICTION.md` §4: *"`list_spaces` … returns the sentence 'No Spaces. Use
/// create_space …' when there are none"*. An SDK must never make an app parse
/// prose, so a non-array payload becomes the empty collection here — once, for
/// every caller.
pub fn expect_array(payload: &Value, unwrapping: Option<&str>) -> Vec<Value> {
    if let Some(array) = payload.as_array() {
        return array.clone();
    }
    if let Some(key) = unwrapping
        && let Some(array) = payload.get(key).and_then(Value::as_array)
    {
        return array.clone();
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// The scripted backend
// ---------------------------------------------------------------------------

/// An in-process backend driven by a JSON script, so the whole control plane
/// is exercised in CI without a live Space.
///
/// The script's responses are **raw MCP `result` objects**, not decoded
/// payloads. That is deliberate: the fake therefore goes through
/// `decode_tool_result` exactly as the live transport does, so the §3 seam is
/// covered by the conformance suite rather than only by a unit test.
///
/// ```text
/// { "script": "cua.control.script/1",
///   "responses": { "<tool>": [ { "content": [...], "isError": false } ] } }
/// ```
///
/// A tool with several scripted responses hands them out in order and repeats
/// the last one forever, which is what a poll loop needs.
pub struct ScriptedTransport {
    responses: HashMap<String, Vec<Value>>,
    tools: Vec<String>,
    calls: Mutex<Vec<(String, Value)>>,
    cursors: Mutex<HashMap<String, usize>>,
}

impl ScriptedTransport {
    pub const SCRIPT_SCHEMA: &'static str = "cua.control.script/1";

    pub fn from_script(script_json: &str) -> Result<Self> {
        let script: Value = serde_json::from_str(script_json)
            .map_err(|error| SpacesError::malformed("script", error.to_string()))?;
        let declared = script.get("script").and_then(Value::as_str).unwrap_or("");
        if declared != Self::SCRIPT_SCHEMA {
            return Err(SpacesError::malformed(
                "script",
                format!("expected {}, got {declared:?}", Self::SCRIPT_SCHEMA),
            ));
        }
        let mut responses: HashMap<String, Vec<Value>> = HashMap::new();
        if let Some(map) = script.get("responses").and_then(Value::as_object) {
            for (tool, value) in map {
                let list = match value {
                    Value::Array(items) => items.clone(),
                    single => vec![single.clone()],
                };
                responses.insert(tool.clone(), list);
            }
        }
        let mut tools: Vec<String> = script
            .get("tools")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_else(|| responses.keys().cloned().collect());
        tools.sort();
        Ok(ScriptedTransport {
            responses,
            tools,
            calls: Mutex::new(Vec::new()),
            cursors: Mutex::new(HashMap::new()),
        })
    }

    /// Every call made so far, in order — tool name and arguments. The honest
    /// way to assert that a roster costs one round trip rather than `1 + N`.
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().map(|c| c.len()).unwrap_or(0)
    }
}

impl ToolTransport for ScriptedTransport {
    fn call_tool(&self, name: &str, arguments: &Value) -> Result<Value> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push((name.to_string(), arguments.clone()));
        }
        let Some(list) = self.responses.get(name) else {
            return Err(SpacesError::ToolFailed {
                tool: name.to_string(),
                message: format!("the scripted backend has no response for {name}"),
            });
        };
        let index = {
            let mut cursors = self
                .cursors
                .lock()
                .map_err(|_| SpacesError::TransportUnavailable("script poisoned".into()))?;
            let cursor = cursors.entry(name.to_string()).or_insert(0);
            let index = (*cursor).min(list.len().saturating_sub(1));
            *cursor += 1;
            index
        };
        decode_tool_result(name, &list[index])
    }

    fn available_tools(&self) -> Result<Vec<String>> {
        Ok(self.tools.clone())
    }
}

// ---------------------------------------------------------------------------
// The in-process transport
// ---------------------------------------------------------------------------

/// The typed client driving the Rust MCP server ([`crate::mcp::McpServer`])
/// in the same process: real JSON-RPC `tools/call` messages, no pipe.
///
/// `ToolTransport` is synchronous, so each call blocks on the server's
/// future. It must not be used from inside a single-threaded async runtime;
/// from a multi-threaded runtime it steps off the worker with
/// `block_in_place`.
pub struct InProcessTransport {
    server: crate::mcp::McpServer,
    runtime: tokio::runtime::Handle,
    next_id: std::sync::atomic::AtomicI64,
}

impl InProcessTransport {
    /// A transport over `server`, running its futures on `runtime`.
    pub fn new(server: crate::mcp::McpServer, runtime: tokio::runtime::Handle) -> Self {
        Self {
            server,
            runtime,
            next_id: std::sync::atomic::AtomicI64::new(1),
        }
    }

    fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let server = self.server.clone();
        let fut = async move { server.handle(message).await };
        let response = match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| self.runtime.block_on(fut)),
            Err(_) => self.runtime.block_on(fut),
        }
        .ok_or_else(|| SpacesError::malformed(method, "no response"))?;
        if let Some(error) = response.get("error") {
            return Err(SpacesError::ToolFailed {
                tool: method.to_string(),
                message: error.to_string(),
            });
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

impl ToolTransport for InProcessTransport {
    fn call_tool(&self, name: &str, arguments: &Value) -> Result<Value> {
        let result = self
            .rpc("tools/call", json!({"name": name, "arguments": arguments}))
            .map_err(|e| match e {
                SpacesError::ToolFailed { message, .. } => SpacesError::ToolFailed {
                    tool: name.to_string(),
                    message,
                },
                other => other,
            })?;
        decode_tool_result(name, &result)
    }

    fn available_tools(&self) -> Result<Vec<String>> {
        let result = self.rpc("tools/list", json!({}))?;
        Ok(result
            .get("tools")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str) -> Value {
        json!({ "content": [{ "type": "text", "text": text }] })
    }

    /// §3. A failing tool arrives shaped like a value; it must leave shaped
    /// like an error.
    #[test]
    fn is_error_becomes_an_error_and_never_a_value() {
        let result = json!({
            "content": [{ "type": "text", "text": "error: no such Space" }],
            "isError": true,
        });
        let error = decode_tool_result("space_bash", &result).unwrap_err();
        assert_eq!(
            error,
            SpacesError::ToolFailed {
                tool: "space_bash".into(),
                message: "error: no such Space".into(),
            }
        );
    }

    #[test]
    fn a_json_payload_is_parsed_and_prose_is_kept_as_a_string() {
        assert_eq!(
            decode_tool_result("list_spaces", &ok("{\"spaces\":[]}")).unwrap(),
            json!({ "spaces": [] })
        );
        assert_eq!(
            decode_tool_result("list_spaces", &ok("No Spaces. Use create_space.")).unwrap(),
            Value::String("No Spaces. Use create_space.".into())
        );
    }

    #[test]
    fn an_empty_payload_is_an_empty_object_rather_than_an_empty_string() {
        assert_eq!(
            decode_tool_result("show_space_pip", &json!({})).unwrap(),
            json!({})
        );
    }

    /// §4. Prose where an array belongs becomes the empty collection, once.
    #[test]
    fn prose_where_an_array_belongs_is_an_empty_collection() {
        let prose = Value::String("No Spaces. Use create_space to make one.".into());
        assert!(expect_array(&prose, Some("spaces")).is_empty());
        assert_eq!(
            expect_array(&json!({ "spaces": [1, 2] }), Some("spaces")).len(),
            2
        );
        assert_eq!(expect_array(&json!([1, 2, 3]), None).len(), 3);
    }

    #[test]
    fn the_scripted_backend_repeats_its_last_response() {
        let transport = ScriptedTransport::from_script(
            r#"{"script":"cua.control.script/1","responses":{
                 "agent_status":[
                   {"content":[{"type":"text","text":"{\"status\":\"running\"}"}]},
                   {"content":[{"type":"text","text":"{\"status\":\"finished\"}"}]}]}}"#,
        )
        .unwrap();
        let states: Vec<String> = (0..3)
            .map(|_| {
                transport.call_tool("agent_status", &json!({})).unwrap()["status"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(states, vec!["running", "finished", "finished"]);
        assert_eq!(transport.call_count(), 3);
    }

    #[test]
    fn a_scripted_is_error_reaches_the_caller_as_an_error() {
        let transport = ScriptedTransport::from_script(
            r#"{"script":"cua.control.script/1","responses":{
                 "space_bash":[{"content":[{"type":"text","text":"error: boom"}],"isError":true}]}}"#,
        )
        .unwrap();
        assert_eq!(
            transport.call_tool("space_bash", &json!({})).unwrap_err(),
            SpacesError::ToolFailed {
                tool: "space_bash".into(),
                message: "error: boom".into()
            }
        );
    }
}
