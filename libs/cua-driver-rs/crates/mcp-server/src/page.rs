//! Cross-platform `page` tool: browser JS execution, text extraction, and DOM querying.
//!
//! The tool itself (name, schema, action dispatch) lives here so every platform
//! gets the same MCP surface. Each platform crate supplies a `PageBackend` impl
//! that routes the read/write primitives to platform-native code:
//!
//! - **macOS** uses Apple Events for Chromium/Safari, CDP for Electron, and
//!   the AX tree as a WKWebView fallback. See
//!   `platform-macos/src/tools/page.rs`.
//! - **Windows** uses UIA TextPattern + FindAll for `get_text` / `query_dom`,
//!   and the shared CDP client (`crate::cdp`) for `execute_javascript`.
//! - **Linux** uses AT-SPI for read paths, CDP for JS exec.
//!
//! `enable_javascript_apple_events` is intentionally macOS-only; the default
//! trait impl returns "not supported on this platform" so non-macOS callers
//! get a clear error instead of a panic.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};

/// Platform-specific backend for the `page` tool.
///
/// All methods take `(pid, window_id)` for consistency across actions.
/// `bundle_id` is resolved by the backend if needed (macOS uses it for
/// AppleScript routing; Windows/Linux ignore it).
#[async_trait]
pub trait PageBackend: Send + Sync {
    /// Returns the visible text of the page (rough analog of
    /// `document.body.innerText`).
    async fn get_text(&self, pid: i32, window_id: u32) -> anyhow::Result<String>;

    /// Find elements matching `css_selector` and return a formatted-text
    /// response (same human-readable shape macOS already emits).
    async fn query_dom(
        &self,
        pid: i32,
        window_id: u32,
        css_selector: &str,
        attributes: &[String],
    ) -> anyhow::Result<String>;

    /// Evaluate `javascript` against the page and return the stringified
    /// result. Backends without a JS path should return an actionable error.
    async fn execute_javascript(
        &self,
        pid: i32,
        window_id: u32,
        javascript: &str,
    ) -> anyhow::Result<String>;

    /// Whether this backend can execute JS at all.  False → the tool short-
    /// circuits with a "not supported here" error; true → the backend may
    /// still return an error at call time (e.g. CDP port not reachable).
    fn supports_javascript(&self) -> bool {
        true
    }

    /// macOS-only: enable "Allow JavaScript from Apple Events" on the named
    /// browser bundle. Default impl returns an unsupported-platform error.
    async fn enable_javascript_apple_events(&self, _bundle_id: &str) -> anyhow::Result<String> {
        anyhow::bail!(
            "enable_javascript_apple_events is only supported on macOS \
             (Chrome/Brave/Edge use Apple Events as the JS backend there)."
        )
    }
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
        description: "Interact with the browser page loaded in a running app. Supports \
            Chrome, Brave, Edge, Safari (via AppleScript on macOS), Electron apps (via CDP), \
            Chromium/Firefox on Windows (via UIA for read; CDP for execute_javascript when \
            --remote-debugging-port is set), and WKWebView/Tauri/AT-SPI fallbacks.\n\n\
            Actions:\n\
            - execute_javascript: Run JS and return the result.\n\
            - get_text: Extract visible text from the page.\n\
            - query_dom: Find elements matching a CSS selector.\n\
            - enable_javascript_apple_events: macOS-only — patch the browser's \
              Preferences to allow JS from Apple Events (Chrome/Brave/Edge, requires user \
              confirmation and a browser restart).".into(),
        input_schema: serde_json::json!({
            "type": "object",
            // `pid` and `window_id` are required for every action except
            // `enable_javascript_apple_events` (which targets a browser by
            // bundle id, not a running process). Enforced per-action in
            // `invoke` rather than via the schema so the JSON-Schema-only
            // path still validates the common case.
            "required": ["action"],
            "properties": {
                "pid": { "type": "integer", "description": "Target process ID." },
                "window_id": { "type": "integer", "description": "Target window ID from list_windows." },
                "action": {
                    "type": "string",
                    "enum": ["execute_javascript", "get_text", "query_dom", "enable_javascript_apple_events"],
                    "description": "Action to perform."
                },
                "javascript": {
                    "type": "string",
                    "description": "JavaScript to execute. Required for execute_javascript."
                },
                "css_selector": {
                    "type": "string",
                    "description": "CSS selector for query_dom (e.g. 'a', 'button', 'input', 'h1'-'h6', 'p', 'img', 'select', '*')."
                },
                "attributes": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Element attributes to include in query_dom results."
                },
                "bundle_id": {
                    "type": "string",
                    "description": "Bundle ID of the browser. Required for enable_javascript_apple_events (macOS only)."
                },
                "user_has_confirmed_enabling": {
                    "type": "boolean",
                    "description": "Must be true to proceed with enable_javascript_apple_events. \
                        This will quit and relaunch the browser."
                }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: false,
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

        // `pid` / `window_id` are resolved per-action: every action except
        // `enable_javascript_apple_events` needs both. We resolve once here
        // so each arm can `?` on the Result and we get matching error text.
        // Narrowing casts use `TryFrom` so out-of-range JSON numbers fail
        // with an actionable error instead of silently truncating to the
        // wrong process / window.
        let resolve_pid = |args: &Value| -> Result<i32, String> {
            let raw = args
                .get("pid")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "Missing required parameter: pid".to_owned())?;
            i32::try_from(raw).map_err(|_| format!("Invalid parameter: pid {raw} out of i32 range"))
        };
        let resolve_window_id = |args: &Value| -> Result<u32, String> {
            let raw = args
                .get("window_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "Missing required parameter: window_id".to_owned())?;
            u32::try_from(raw)
                .map_err(|_| format!("Invalid parameter: window_id {raw} out of u32 range"))
        };

        let (pid, window_id) = if action == "enable_javascript_apple_events" {
            (0i32, 0u32) // unused
        } else {
            let pid = match resolve_pid(&args) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(e),
            };
            let window_id = match resolve_window_id(&args) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(e),
            };
            (pid, window_id)
        };

        match action.as_str() {
            "enable_javascript_apple_events" => {
                let confirmed = args
                    .get("user_has_confirmed_enabling")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !confirmed {
                    return ToolResult::error(
                        "Set user_has_confirmed_enabling=true to proceed. \
                         This will quit and relaunch the browser.",
                    );
                }
                let bid = match args.get("bundle_id").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s.to_owned(),
                    _ => {
                        return ToolResult::error(
                            "bundle_id is required for enable_javascript_apple_events",
                        )
                    }
                };
                match self.backend.enable_javascript_apple_events(&bid).await {
                    Ok(msg) => ToolResult::text(msg),
                    Err(e) => ToolResult::error(format!("{e}")),
                }
            }

            "execute_javascript" => {
                if !self.backend.supports_javascript() {
                    return ToolResult::error(
                        "execute_javascript is not supported by this platform's page backend. \
                         Use get_text or query_dom for read-only access.",
                    );
                }
                let js = match args.get("javascript").and_then(|v| v.as_str()) {
                    Some(v) => v.to_owned(),
                    None => return ToolResult::error("Missing required parameter: javascript"),
                };
                match self.backend.execute_javascript(pid, window_id, &js).await {
                    Ok(result) => ToolResult::text(result),
                    Err(e) => ToolResult::error(format!("{e}")),
                }
            }

            "get_text" => match self.backend.get_text(pid, window_id).await {
                Ok(text) => ToolResult::text(text),
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
                    Ok(text) => ToolResult::text(text),
                    Err(e) => ToolResult::error(format!("DOM query failed: {e}")),
                }
            }

            other => ToolResult::error(format!("Unknown action: {other}")),
        }
    }
}
