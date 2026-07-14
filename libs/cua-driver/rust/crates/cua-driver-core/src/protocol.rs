//! MCP JSON-RPC 2.0 protocol types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Request ──────────────────────────────────────────────────────────────────

/// An incoming JSON-RPC 2.0 request or notification.
#[derive(Debug, Deserialize)]
pub struct Request {
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// Absent on notifications.
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

impl Request {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    pub fn tool_call(&self) -> anyhow::Result<ToolCall> {
        let params = self.params.as_ref().ok_or_else(|| anyhow::anyhow!("missing params"))?;
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing tool name"))?
            .to_owned();
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));
        Ok(ToolCall { name, args })
    }

    /// Extract the small, declared subset of MCP initialize metadata that a
    /// transport observer may use. The complete initialize payload is never
    /// retained or forwarded: arbitrary `_meta`, capability payloads, and
    /// client-defined extension values are deliberately ignored.
    pub fn initialize_metadata(&self) -> Option<InitializeMetadata> {
        if self.method != "initialize" {
            return None;
        }

        let params = self.params.as_ref();
        let protocol_version = params
            .and_then(|p| p.get("protocolVersion"))
            .and_then(Value::as_str)
            .and_then(bounded_metadata_value);
        let client_info = params.and_then(|p| p.get("clientInfo"));
        let client_name = client_info
            .and_then(|v| v.get("name"))
            .and_then(Value::as_str)
            .and_then(bounded_metadata_value);
        let client_version = client_info
            .and_then(|v| v.get("version"))
            .and_then(Value::as_str)
            .and_then(bounded_metadata_value);

        let capabilities = params.and_then(|p| p.get("capabilities"));
        let capability_flags = ClientCapabilityFlags {
            tools: capability_declared(capabilities, "tools"),
            roots: capability_declared(capabilities, "roots"),
            sampling: capability_declared(capabilities, "sampling"),
            experimental: capability_declared(capabilities, "experimental"),
            elicitation_form: nested_capability_declared(capabilities, "elicitation", "form"),
            elicitation_url: nested_capability_declared(capabilities, "elicitation", "url"),
        };

        let reported_agent_context = params
            .and_then(|p| p.get("_meta"))
            .and_then(|m| m.get("ai.cua/agent-context"))
            .filter(|v| v.is_object())
            .map(|ctx| ReportedAgentContext {
                provider: ctx
                    .get("provider")
                    .and_then(Value::as_str)
                    .and_then(bounded_metadata_value),
                model: ctx
                    .get("model")
                    .and_then(Value::as_str)
                    .and_then(bounded_metadata_value),
                agent_name: ctx
                    .get("agent_name")
                    .and_then(Value::as_str)
                    .and_then(bounded_metadata_value),
                agent_version: ctx
                    .get("agent_version")
                    .and_then(Value::as_str)
                    .and_then(bounded_metadata_value),
            })
            .filter(ReportedAgentContext::has_any_value);

        Some(InitializeMetadata {
            protocol_version,
            client_name,
            client_version,
            capability_flags,
            reported_agent_context,
        })
    }
}

/// Declared and bounded metadata from one MCP initialize request.
///
/// Values remain self-reported. A product telemetry layer must normalize them
/// through its own allowlists before emission.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InitializeMetadata {
    pub protocol_version: Option<String>,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub capability_flags: ClientCapabilityFlags,
    pub reported_agent_context: Option<ReportedAgentContext>,
}

/// Presence-only capability flags. Capability values themselves are ignored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientCapabilityFlags {
    pub tools: bool,
    pub roots: bool,
    pub sampling: bool,
    pub experimental: bool,
    pub elicitation_form: bool,
    pub elicitation_url: bool,
}

/// Values explicitly reported under
/// `initialize.params._meta["ai.cua/agent-context"]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportedAgentContext {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub agent_name: Option<String>,
    pub agent_version: Option<String>,
}

impl ReportedAgentContext {
    fn has_any_value(&self) -> bool {
        self.provider.is_some()
            || self.model.is_some()
            || self.agent_name.is_some()
            || self.agent_version.is_some()
    }
}

const MAX_METADATA_CHARS: usize = 128;

fn bounded_metadata_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let value: String = trimmed
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_METADATA_CHARS)
        .collect();
    (!value.is_empty()).then_some(value)
}

fn capability_declared(capabilities: Option<&Value>, name: &str) -> bool {
    capabilities
        .and_then(|v| v.as_object())
        .is_some_and(|caps| caps.contains_key(name))
}

fn nested_capability_declared(
    capabilities: Option<&Value>,
    parent: &str,
    child: &str,
) -> bool {
    capabilities
        .and_then(|v| v.get(parent))
        .and_then(Value::as_object)
        .is_some_and(|value| value.contains_key(child))
}

pub struct ToolCall {
    pub name: String,
    pub args: Value,
}

// ── Response ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(flatten)]
    pub body: ResponseBody,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ResponseBody {
    Result { result: Value },
    Error { error: RpcError },
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, body: ResponseBody::Result { result } }
    }

    pub fn error(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            body: ResponseBody::Error { error: RpcError { code, message: message.into() } },
        }
    }

    pub fn parse_error() -> Self {
        Self::error(Value::Null, -32700, "Parse error")
    }

    pub fn method_not_found(id: Value, method: &str) -> Self {
        Self::error(id, -32601, format!("Unknown method: {method}"))
    }
}

// ── Tool result content ───────────────────────────────────────────────────────

/// A single item in the `content` array of a `tools/call` result.
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Content {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        annotations: Option<Value>,
    },
    Image {
        data: String,     // base64-encoded PNG
        #[serde(rename = "mimeType")]
        mime_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        annotations: Option<Value>,
    },
}

impl Content {
    pub fn text(text: impl Into<String>) -> Self {
        Content::Text { text: text.into(), annotations: None }
    }

    pub fn image_png(data_base64: String) -> Self {
        Content::Image { data: data_base64, mime_type: "image/png".into(), annotations: None }
    }

    pub fn image_jpeg(data_base64: String) -> Self {
        Content::Image { data: data_base64, mime_type: "image/jpeg".into(), annotations: None }
    }
}

/// The value placed in `result` for a `tools/call` response.
#[derive(Debug, Serialize, Default)]
pub struct ToolResult {
    pub content: Vec<Content>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(rename = "structuredContent", skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
}

impl ToolResult {
    pub fn text(msg: impl Into<String>) -> Self {
        Self { content: vec![Content::text(msg)], ..Default::default() }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self { content: vec![Content::text(msg)], is_error: Some(true), ..Default::default() }
    }

    pub fn with_structured(mut self, v: Value) -> Self {
        self.structured_content = Some(v);
        self
    }
}

// ── Initialize result ─────────────────────────────────────────────────────────

pub fn initialize_result() -> Value {
    serde_json::json!({
        "protocolVersion": "2025-06-18",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "cua-driver", "version": env!("CARGO_PKG_VERSION") },
        "instructions": agent_instructions()
    })
}

/// MCP `instructions` (`InitializeResult.instructions`) sent to every
/// connecting client. The spec frames this as a "hint... MAY be added
/// to the system prompt" — eager, every-turn cost. We keep it under
/// the community-recommended ~200-word ceiling and host the long-form
/// workflow in `Skills/cua-driver/SKILL.md`.
///
/// Templated per-host: the accessibility-tree provider name (AX on
/// macOS, UIA on Windows, AT-SPI on Linux) is injected so a connecting
/// agent only sees the path that applies, not all three. Same pattern
/// Goose uses in its `ComputerController` extension and Open
/// Interpreter uses for its system message.
fn agent_instructions() -> String {
    let (tree_kind, platform_skill_pointer) = if cfg!(target_os = "macos") {
        (
            "AX (Accessibility)",
            "MACOS.md (no-foreground contract, AXMenuBar navigation, SkyLight click dispatch)",
        )
    } else if cfg!(target_os = "windows") {
        (
            "UIA (UI Automation)",
            "WINDOWS.md (UIA tree, UWP / ApplicationFrameHost hosting, Session 0 isolation)",
        )
    } else {
        (
            "AT-SPI",
            "LINUX.md (X11/Wayland status, AT-SPI bus, BETA-level support)",
        )
    };

    format!(
        r#"cua-driver: cross-platform background computer-use automation.

Tools let you interact with any app without stealing keyboard focus or moving the visible cursor. Prefer element_index ({tree_kind}) paths over pixel coordinates — they work on backgrounded/hidden windows.

Workflow per turn:
0. start_session(session) once at the start of a run → declares THIS run's identity (a stable id you choose, e.g. "research-1"). Pass that same `session` on every action below. It owns your agent cursor (a distinct color per id) and follows the run across apps/windows. End with end_session(session) when done. Concurrent runs/subagents each use their OWN `session`. (Omitting `session` still works, just with no cursor.)
1. launch_app  → idempotent, returns pid + windows array in one call. Pass creates_new_application_instance:true if another run may touch the same app, so you get your own window.
2. (skip list_windows when launch_app already returned a single window)
3. get_window_state(pid, window_id) → refresh the {tree_kind} snapshot, get element indices
4. click/type_text/press_key using element_index from step 3 (+ your `session`)
5. get_window_state(pid, window_id) again → verify the action landed

Agent cursor: a per-SESSION overlay cursor visualises where a run is acting without moving the real pointer. It is shown only for a DECLARED session (pass `session`), is color-coded by the session id, and is removed by end_session or the idle-TTL. The same id over MCP, the CLI, or the raw socket drives the same cursor. set_agent_cursor_* tools hide/show/customise it. Note: a pure accessibility-action (element_index) click snaps the cursor with a brief pulse on its first action rather than a long glide, so it can be easy to miss — issue a pixel click or move_cursor first for a visibly gliding demo/recording.

If a `cua-driver` skill is loaded in your harness (Claude Code / Codex / OpenClaw / OpenCode dirs), prefer its detailed workflow — SKILL.md plus {platform_skill_pointer}. Install with `cua-driver skills install` if not yet present."#
    )
}

#[cfg(test)]
mod image_mime_type_tests {
    use super::Content;

    /// Surface 7 contract: image parts serialize an explicit `mimeType` field
    /// (not `mime_type`, not `format`) so MCP consumers don't have to sniff
    /// magic bytes off the base64 PNG/JPEG to know what they're holding.
    /// This locks in the JSON shape — any rename or drop breaks here first.
    #[test]
    fn image_png_serializes_with_explicit_mime_type() {
        let c = Content::image_png("ZmFrZQ==".into());
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("image"));
        assert_eq!(v.get("mimeType").and_then(|t| t.as_str()), Some("image/png"));
        assert_eq!(v.get("data").and_then(|t| t.as_str()), Some("ZmFrZQ=="));
    }

    #[test]
    fn image_jpeg_serializes_with_explicit_mime_type() {
        let c = Content::image_jpeg("ZmFrZQ==".into());
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("image"));
        assert_eq!(v.get("mimeType").and_then(|t| t.as_str()), Some("image/jpeg"));
    }

    /// Text parts must not grow a mimeType field — the field belongs to
    /// image parts only. Guards against an accidental serde_json renamer
    /// that emits a shared shape across the enum.
    #[test]
    fn text_does_not_carry_mime_type() {
        let c = Content::text("hi");
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("text"));
        assert!(v.get("mimeType").is_none(), "text content must not carry mimeType");
    }
}

#[cfg(test)]
mod initialize_metadata_tests {
    use super::*;

    fn request(params: Value) -> Request {
        serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": params,
        }))
        .expect("initialize request")
    }

    #[test]
    fn extracts_only_declared_initialize_fields() {
        let req = request(serde_json::json!({
            "protocolVersion": "2025-06-18",
            "clientInfo": { "name": "Claude Code", "version": "1.2.3", "secret": "ignore" },
            "capabilities": {
                "roots": { "listChanged": true, "arbitrary": "ignored" },
                "sampling": {},
                "elicitation": { "form": {}, "url": { "payload": "ignored" } },
                "experimental": { "anything": "ignored" }
            },
            "_meta": {
                "ai.cua/agent-context": {
                    "provider": "anthropic",
                    "model": "claude-sonnet-4-5",
                    "agent_name": "claude-code",
                    "agent_version": "1.x",
                    "prompt": "must not be extracted"
                },
                "unrelated": { "task": "must not be extracted" }
            },
            "task": "must not be extracted"
        }));

        let metadata = req.initialize_metadata().expect("initialize metadata");
        assert_eq!(metadata.protocol_version.as_deref(), Some("2025-06-18"));
        assert_eq!(metadata.client_name.as_deref(), Some("Claude Code"));
        assert_eq!(metadata.client_version.as_deref(), Some("1.2.3"));
        assert!(metadata.capability_flags.roots);
        assert!(metadata.capability_flags.sampling);
        assert!(metadata.capability_flags.experimental);
        assert!(metadata.capability_flags.elicitation_form);
        assert!(metadata.capability_flags.elicitation_url);
        assert!(!metadata.capability_flags.tools);

        let agent = metadata
            .reported_agent_context
            .as_ref()
            .expect("reported agent context");
        assert_eq!(agent.provider.as_deref(), Some("anthropic"));
        assert_eq!(agent.model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(agent.agent_name.as_deref(), Some("claude-code"));
        assert_eq!(agent.agent_version.as_deref(), Some("1.x"));

        let debug = format!("{metadata:?}");
        for forbidden in ["secret", "prompt", "task", "arbitrary", "payload"] {
            assert!(!debug.contains(forbidden), "metadata leaked {forbidden}: {debug}");
        }
    }

    #[test]
    fn bounds_reported_strings_and_drops_empty_values() {
        let long_model = "m".repeat(MAX_METADATA_CHARS + 50);
        let req = request(serde_json::json!({
            "clientInfo": { "name": "  client\nname  ", "version": "   " },
            "_meta": { "ai.cua/agent-context": { "model": long_model } }
        }));
        let metadata = req.initialize_metadata().unwrap();
        assert_eq!(metadata.client_name.as_deref(), Some("clientname"));
        assert!(metadata.client_version.is_none());
        assert_eq!(
            metadata
                .reported_agent_context
                .unwrap()
                .model
                .unwrap()
                .chars()
                .count(),
            MAX_METADATA_CHARS
        );
    }

    #[test]
    fn non_initialize_requests_have_no_initialize_metadata() {
        let req: Request = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": { "clientInfo": { "name": "not-an-initialize" } }
        }))
        .unwrap();
        assert!(req.initialize_metadata().is_none());
    }
}
