// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cross-platform `page` tool: read-only page text extraction and DOM querying.
//!
//! The tool itself (name, schema, action dispatch) lives here so every platform
//! gets the same MCP surface. Each platform crate supplies a `PageBackend` impl
//! that routes the read primitives to platform-native code:
//!
//! - **macOS** uses Apple Events for Chromium/Safari, CDP for Electron, and
//!   the AX tree as a WKWebView fallback. See
//!   `platform-macos/src/tools/page.rs`.
//! - **Windows** uses UIA TextPattern + FindAll.
//! - **Linux** uses AT-SPI.
//!
//! The tool is read-only. Its page actions (run JavaScript, click, type,
//! enable Apple Events JavaScript) were removed; use `get_browser_state` and
//! the typed `browser_*` tools to act on a page.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};

pub type PageReadResult = ToolResult;

/// Platform-specific backend for the `page` tool.
///
/// All methods take `(pid, window_id)` for consistency across actions.
/// `bundle_id` is resolved by the backend if needed (macOS uses it for
/// AppleScript routing; Windows/Linux ignore it).
#[async_trait]
pub trait PageBackend: Send + Sync {
    /// Returns the visible text of the page (rough analog of
    /// `document.body.innerText`) plus optional backend provenance.
    async fn get_text(&self, pid: i32, window_id: u64) -> anyhow::Result<PageReadResult>;

    /// Find elements matching `css_selector` and return the legacy formatted
    /// text response plus optional backend provenance.
    async fn query_dom(
        &self,
        pid: i32,
        window_id: u64,
        css_selector: &str,
        attributes: &[String],
    ) -> anyhow::Result<PageReadResult>;
}

/// The cross-platform `page` MCP tool.  Holds a backend trait object the
/// host platform constructs and passes in at registration time.
pub struct PageTool {
    backend: Arc<dyn PageBackend>,
}

impl PageTool {
    pub fn new(backend: Arc<dyn PageBackend>) -> Self {
        Self { backend }
    }
}

fn def() -> &'static ToolDef {
    static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
    DEF.get_or_init(|| ToolDef {
        name: "page".into(),
        description: "Read-only legacy browser compatibility tool: read a page's text or \
            query its DOM. Prefer get_browser_state and the typed browser_* tools for exact \
            targeting, endpoint ownership, and consent; use this tool to read Safari and \
            Firefox pages, which the typed tools do not cover. It cannot run JavaScript, \
            click, or type: those page actions were removed, so act with the browser_* \
            tools. Supports Chrome, Brave, Edge, Safari (via AppleScript on macOS), Electron \
            apps (via CDP), Chromium/Firefox on Windows (via UIA), and \
            WKWebView/Tauri/AT-SPI fallbacks.\n\n\
            Actions:\n\
            - get_text: Extract visible text from the page. On macOS, structuredContent.source \
              identifies javascript, cdp, or ax_fallback; a successful AX fallback preserves \
              the triggering error and truncation state in structuredContent.\n\
            - query_dom: Find elements matching a CSS selector, with the same macOS fallback \
              metadata contract as get_text.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "pid": { "type": "integer", "description": "Target process ID." },
                "window_id": { "type": "integer", "description": "Target window ID from list_windows." },
                "action": {
                    "type": "string",
                    "enum": ["get_text", "query_dom"],
                    "description": "Action to perform."
                },
                "css_selector": {
                    "type": "string",
                    "description": "CSS selector for query_dom (e.g. 'a', 'button', 'input', 'h1'-'h6', 'p', 'img', 'select', '*')."
                },
                "attributes": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Element attributes to include in query_dom results."
                }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true,
    })
}

#[async_trait]
impl Tool for PageTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(v) => v.to_owned(),
            None => return ToolResult::error("Missing required parameter: action"),
        };

        // PID narrowing uses `TryFrom` so out-of-range JSON numbers fail
        // instead of silently truncating. Window IDs remain u64 because
        // native Wayland accessibility providers can legitimately exceed u32.
        let resolve_pid = |args: &Value| -> Result<i32, String> {
            let raw = args
                .get("pid")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "Missing required parameter: pid".to_owned())?;
            i32::try_from(raw).map_err(|_| format!("Invalid parameter: pid {raw} out of i32 range"))
        };
        let resolve_window_id = |args: &Value| -> Result<u64, String> {
            args.get("window_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: window_id".to_owned())
        };

        let pid = match resolve_pid(&args) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(e),
        };
        let window_id = match resolve_window_id(&args) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(e),
        };

        match action.as_str() {
            "get_text" => match self.backend.get_text(pid, window_id).await {
                Ok(result) => result,
                Err(e) => ToolResult::error(format!("Page text extraction failed: {e}")),
            },

            "query_dom" => {
                let selector = args
                    .get("css_selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("*")
                    .to_owned();
                let attributes: Vec<String> = args
                    .get("attributes")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                match self
                    .backend
                    .query_dom(pid, window_id, &selector, &attributes)
                    .await
                {
                    Ok(result) => result,
                    Err(e) => ToolResult::error(format!("DOM query failed: {e}")),
                }
            }

            "execute_javascript"
            | "click_element"
            | "insert_text"
            | "type_keystrokes"
            | "enable_javascript_apple_events" => ToolResult::error(format!(
                "page action {action:?} was removed; `page` only reads (get_text, query_dom). \
                 Use get_browser_state and the typed browser_* tools to act on a page."
            )),

            other => ToolResult::error(format!("Unknown action: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ProvenanceBackend;

    #[async_trait]
    impl PageBackend for ProvenanceBackend {
        async fn get_text(&self, _pid: i32, _window_id: u64) -> anyhow::Result<PageReadResult> {
            Ok(
                ToolResult::text("legacy payload".to_owned()).with_structured(serde_json::json!({
                    "source": "ax_fallback",
                    "fallback_error": "JavaScript from Apple Events is disabled",
                    "truncated": true
                })),
            )
        }

        async fn query_dom(
            &self,
            _pid: i32,
            _window_id: u64,
            _css_selector: &str,
            _attributes: &[String],
        ) -> anyhow::Result<PageReadResult> {
            Ok(PageReadResult::text(String::new()))
        }
    }

    #[tokio::test]
    async fn read_preserves_text_and_attaches_backend_provenance() {
        let tool = PageTool::new(Arc::new(ProvenanceBackend));
        let result = tool
            .invoke(serde_json::json!({
                "pid": 42,
                "window_id": 7,
                "action": "get_text"
            }))
            .await;

        assert!(matches!(
            &result.content[0],
            crate::protocol::Content::Text { text, .. } if text == "legacy payload"
        ));
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({
                "source": "ax_fallback",
                "fallback_error": "JavaScript from Apple Events is disabled",
                "truncated": true
            }))
        );
    }

    #[test]
    fn the_schema_advertises_only_the_two_read_actions() {
        let schema = &def().input_schema;
        assert_eq!(
            schema["properties"]["action"]["enum"],
            serde_json::json!(["get_text", "query_dom"])
        );
        let properties = schema["properties"].as_object().unwrap();
        for removed in [
            "javascript",
            "selector",
            "text",
            "cdp_port",
            "target_url_contains",
            "bundle_id",
            "user_has_confirmed_enabling",
        ] {
            assert!(!properties.contains_key(removed), "{removed}");
        }
        assert_eq!(schema["additionalProperties"], false);
        assert!(def().open_world);
    }

    #[tokio::test]
    async fn the_removed_page_actions_are_refused_without_reaching_the_backend() {
        let tool = PageTool::new(Arc::new(ProvenanceBackend));
        for action in [
            "execute_javascript",
            "click_element",
            "insert_text",
            "type_keystrokes",
            "enable_javascript_apple_events",
        ] {
            let result = tool
                .invoke(serde_json::json!({"pid": 42, "window_id": 7, "action": action}))
                .await;
            assert_eq!(result.is_error, Some(true), "{action}");
            assert!(
                serde_json::to_string(&result)
                    .unwrap()
                    .contains("was removed"),
                "{action}"
            );
        }
    }
}
