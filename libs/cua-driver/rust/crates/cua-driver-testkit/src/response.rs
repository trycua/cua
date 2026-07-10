//! Normalized tool response — the common shape both transports return.

use serde_json::Value;

/// A tool-call result, normalized across the MCP and CLI transports.
///
/// MCP returns `{"result":{"content":[{"text":…}],"structuredContent":{…},
/// "isError":bool}}`; the CLI prints `structuredContent` (or the text) directly.
/// Each transport builds a `ToolResponse` with the same accessors below, so test
/// assertions never branch on transport.
pub struct ToolResponse {
    /// Human-readable text (the MCP `content[0].text`, or CLI stdout).
    text: String,
    /// The structured payload (`structuredContent`), or `Null` if none.
    structured: Value,
    /// Whether the call reported an error.
    is_error: bool,
    /// The raw underlying value, for the rare assertion that needs it.
    pub raw: Value,
}

impl ToolResponse {
    pub(crate) fn new(text: String, structured: Value, is_error: bool, raw: Value) -> Self {
        Self {
            text,
            structured,
            is_error,
            raw,
        }
    }

    /// Build from an MCP JSON-RPC response envelope.
    pub(crate) fn from_mcp(raw: Value) -> Self {
        let text = raw["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let structured = raw["result"]["structuredContent"].clone();
        let is_error =
            raw["result"]["isError"].as_bool().unwrap_or(false) || raw.get("error").is_some();
        Self::new(text, structured, is_error, raw)
    }

    /// The result text. Empty string when absent.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The structured payload. `Null` when the tool returned none — index into
    /// it directly (`resp.structured()["screen_width"]`).
    pub fn structured(&self) -> &Value {
        &self.structured
    }

    /// The full accessibility tree text from `get_window_state`.
    ///
    /// Older/direct MCP responses often put the rendered tree in the text content,
    /// while daemon-proxied responses carry a short summary in text and the tree
    /// in `structuredContent.tree_markdown`.
    pub fn tree_text(&self) -> &str {
        self.structured
            .get("tree_markdown")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.text)
    }

    /// Whether the call errored (MCP `isError`/`error`, or CLI nonzero exit).
    pub fn is_error(&self) -> bool {
        self.is_error
    }

    // ── Best-effort-background ladder accessors ──────────────────────────────
    // Action tools (type_text / click / drag / scroll) report which rung
    // delivered (`path`) and whether the driver could confirm the effect
    // (`verified`). These let the modality matrix assert per (surface, rung)
    // outcomes instead of scraping result text.

    /// The delivery rung that ran: `"ax" | "cgevent" | "cgevent_fg" |
    /// "key_events" | "key_events_fg"`. `None` when the tool reports no `path`.
    pub fn path(&self) -> Option<&str> {
        self.structured.get("path").and_then(Value::as_str)
    }

    /// Whether the driver confirmed the effect via read-back. `Some(true)` only
    /// when verified; `Some(false)` = dispatched-but-unconfirmed (the caller
    /// must confirm via screenshot — e.g. a click, or a Catalyst type); `None`
    /// when the tool doesn't carry the field.
    pub fn verified(&self) -> Option<bool> {
        self.structured.get("verified").and_then(Value::as_bool)
    }

    /// `get_window_state` degraded flag: an AX walk that ran but returned zero
    /// actionable elements (non-AX surface / tree not ready). `false` when absent.
    pub fn degraded(&self) -> bool {
        self.structured
            .get("degraded")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}
