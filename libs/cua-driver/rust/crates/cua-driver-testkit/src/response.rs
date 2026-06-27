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
        Self { text, structured, is_error, raw }
    }

    /// Build from an MCP JSON-RPC response envelope.
    pub(crate) fn from_mcp(raw: Value) -> Self {
        let text = raw["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let structured = raw["result"]["structuredContent"].clone();
        let is_error = raw["result"]["isError"].as_bool().unwrap_or(false)
            || raw.get("error").is_some();
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

    /// Whether the call errored (MCP `isError`/`error`, or CLI nonzero exit).
    pub fn is_error(&self) -> bool {
        self.is_error
    }
}
