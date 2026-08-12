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
            .or_else(|| raw["error"]["message"].as_str())
            .or_else(|| raw["error"].as_str())
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

    // ── Action-result accessors ───────────────────────────────────────────────
    // Action tools expose only the narrow public truth contract. Tests assert
    // the stable semantic route/effect instead of depending on platform
    // transport names or a lossy verification boolean.

    /// The strongest effect the driver can substantiate for an action.
    pub fn action_effect(&self) -> Option<&str> {
        self.structured.get("effect").and_then(Value::as_str)
    }

    /// The stable semantic route used by the action.
    pub fn action_route(&self) -> Option<&str> {
        self.structured.get("route").and_then(Value::as_str)
    }

    /// The delivery mode the driver actually used, when applicable.
    pub fn action_delivery_mode(&self) -> Option<&str> {
        self.structured
            .get("delivery")
            .and_then(|delivery| delivery.get("mode"))
            .and_then(Value::as_str)
    }

    /// `get_window_state` degraded flag: an AX walk that ran but returned zero
    /// actionable elements (non-AX surface / tree not ready). `false` when absent.
    pub fn degraded(&self) -> bool {
        self.structured
            .get("degraded")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Snapshot handle paired with numeric element indices. Element-targeted
    /// calls in the 0.17 contract must send this value with `element_index`.
    pub fn snapshot_id(&self) -> &str {
        self.structured
            .get("snapshot_id")
            .and_then(Value::as_str)
            .expect("get_window_state response must carry snapshot_id")
    }
}

#[cfg(test)]
mod tests {
    use super::ToolResponse;

    #[test]
    fn mcp_error_text_accepts_object_and_string_envelopes() {
        let object = ToolResponse::from_mcp(serde_json::json!({
            "error": { "message": "object error" }
        }));
        assert!(object.is_error());
        assert_eq!(object.text(), "object error");

        let string = ToolResponse::from_mcp(serde_json::json!({
            "error": "string error"
        }));
        assert!(string.is_error());
        assert_eq!(string.text(), "string error");
    }
}
