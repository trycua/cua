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
1. launch_app  → idempotent, returns pid + windows array in one call
2. (skip list_windows when launch_app already returned a single window)
3. get_window_state(pid, window_id) → refresh the {tree_kind} snapshot, get element indices
4. click/type_text/press_key using element_index from step 3
5. get_window_state(pid, window_id) again → verify the action landed

Agent cursor: set_agent_cursor_* tools visualise where the agent is acting without affecting the real mouse pointer.

If a `cua-driver` skill is loaded in your harness (Claude Code / Codex / OpenClaw / OpenCode dirs), prefer its detailed workflow — SKILL.md plus {platform_skill_pointer}. Install with `cua-driver skills install` if not yet present."#
    )
}
