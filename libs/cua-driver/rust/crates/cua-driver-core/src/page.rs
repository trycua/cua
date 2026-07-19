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
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use crate::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};

const LEGACY_PAGE_MUTATIONS_ENV: &str = "CUA_DRIVER_ENABLE_LEGACY_PAGE_MUTATIONS";

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn is_mutating_action(action: &str) -> bool {
    matches!(
        action,
        "execute_javascript"
            | "click_element"
            | "insert_text"
            | "type_keystrokes"
            | "enable_javascript_apple_events"
    )
}

/// Result of a `click_element` call. The backend is responsible for driving
/// the cursor overlay before returning; `screen_x` / `screen_y` are reported
/// back to the caller for diagnostics + downstream chaining.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickElementResult {
    /// Screen-space x of the element center, after cursor animation landed.
    pub screen_x: f64,
    /// Screen-space y of the element center.
    pub screen_y: f64,
    /// Viewport-space x (page-relative, before scroll offset compensation).
    pub viewport_x: f64,
    /// Viewport-space y.
    pub viewport_y: f64,
    /// Free-form message for the agent (e.g. "Clicked button.submit").
    pub message: String,
}

/// Platform-specific backend for the `page` tool.
///
/// All methods take `(pid, window_id)` for consistency across actions.
/// `bundle_id` is resolved by the backend if needed (macOS uses it for
/// AppleScript routing; Windows/Linux ignore it).
#[async_trait]
pub trait PageBackend: Send + Sync {
    /// Returns the visible text of the page (rough analog of
    /// `document.body.innerText`).
    async fn get_text(&self, pid: i32, window_id: u64) -> anyhow::Result<String>;

    /// Find elements matching `css_selector` and return a formatted-text
    /// response (same human-readable shape macOS already emits).
    async fn query_dom(
        &self,
        pid: i32,
        window_id: u64,
        css_selector: &str,
        attributes: &[String],
    ) -> anyhow::Result<String>;

    /// Evaluate `javascript` against the page and return the stringified
    /// result. Backends without a JS path should return an actionable error.
    async fn execute_javascript(
        &self,
        pid: i32,
        window_id: u64,
        javascript: &str,
    ) -> anyhow::Result<String>;

    /// Evaluate `javascript` against an explicitly selected CDP page.
    ///
    /// Backends that do not expose per-page CDP routing keep their legacy
    /// behavior only when no explicit target was requested. A supplied port
    /// or URL hint must never be silently ignored.
    async fn execute_javascript_targeted(
        &self,
        pid: i32,
        window_id: u64,
        javascript: &str,
        cdp_port: Option<u16>,
        target_url_contains: Option<&str>,
    ) -> anyhow::Result<String> {
        if cdp_port.is_some() || target_url_contains.is_some() {
            anyhow::bail!(
                "targeted execute_javascript is not implemented on this platform's page backend"
            );
        }
        self.execute_javascript(pid, window_id, javascript).await
    }

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

    /// Click an element identified by a CSS selector, animating the agent
    /// cursor to the element's on-screen center before firing the click.
    ///
    /// The backend is responsible for: (1) running JS to derive the
    /// element's screen coordinates; (2) driving the cursor overlay
    /// (`pin_above` + `animate_cursor_to` + `ClickPulse`); (3) firing
    /// `element.click()` via JS; (4) returning the coords for diagnostics.
    ///
    /// Default impl returns an actionable "not implemented" error so
    /// non-Windows backends keep compiling until they're wired up.
    async fn click_element(
        &self,
        _pid: i32,
        _window_id: u64,
        _selector: &str,
    ) -> anyhow::Result<ClickElementResult> {
        anyhow::bail!(
            "click_element is not yet implemented on this platform's page \
             backend. Use execute_javascript with `el.click()` directly \
             (no visible cursor animation)."
        )
    }

    /// Type `text` into whatever currently holds DOM focus, as a stream of
    /// real per-character keystroke events rather than a one-shot DOM write.
    ///
    /// This is the durable rung for rich-text contenteditable editors
    /// (Draft.js/Lexical/Slate-style composers) whose own state
    /// reconciliation can silently discard a synthetic `execCommand` or
    /// `innerText =` write on the next render — a keystroke stream is what
    /// those editors' own input pipeline actually observes. Plain
    /// `<input>`/`<textarea>` fields don't need this; `execute_javascript`
    /// with `el.value = ...` + a dispatched `input` event is cheaper there.
    ///
    /// Caller is responsible for focusing the target element first (e.g. a
    /// prior `click_element` or `execute_javascript` with `el.click()`).
    ///
    /// Default impl returns an actionable "not implemented" error so
    /// backends without a keystroke-capable path keep compiling.
    /// `cdp_port`, when given, is used directly instead of auto-discovering
    /// a port from `pid` — needed when auto-discovery can't confirm the
    /// port itself (e.g. a CDP endpoint opened via the browser's own
    /// "allow remote debugging" toggle on an already-running profile,
    /// rather than a launch-time flag; that endpoint may not answer the
    /// classic `/json` probe auto-discovery relies on).
    ///
    /// `target_url_contains`, when given, picks the browser tab whose URL
    /// contains this substring instead of whichever tab the backend finds
    /// first — needed on a multi-tab browser, since there's no built-in
    /// relationship between `window_id` and which tab a CDP call reaches.
    async fn type_keystrokes(
        &self,
        _pid: i32,
        _window_id: u64,
        _text: &str,
        _cdp_port: Option<u16>,
        _target_url_contains: Option<&str>,
    ) -> anyhow::Result<String> {
        anyhow::bail!(
            "type_keystrokes is not implemented on this platform's page \
             backend. Fall back to execute_javascript with \
             document.execCommand('insertText', ...) for plain inputs."
        )
    }

    /// Insert `text` at whatever currently holds DOM focus in a single
    /// operation — no synthesized key events, but (unlike a JS-level
    /// `execCommand`/`innerText =` write) it's a native input-pipeline
    /// operation the browser treats similarly to an IME composition commit,
    /// which rich-text editors already have to handle correctly. Cheaper
    /// than `type_keystrokes` and often durable enough on its own; escalate
    /// to `type_keystrokes` only if this rung's write still gets discarded
    /// or the editor needs to observe real keydown/keyup events.
    ///
    /// Caller is responsible for focusing the target element first.
    /// `cdp_port` — see `type_keystrokes`.
    ///
    /// Default impl returns an actionable "not implemented" error so
    /// backends without this path keep compiling.
    async fn insert_text(
        &self,
        _pid: i32,
        _window_id: u64,
        _text: &str,
        _cdp_port: Option<u16>,
        _target_url_contains: Option<&str>,
    ) -> anyhow::Result<String> {
        anyhow::bail!(
            "insert_text is not implemented on this platform's page backend. \
             Fall back to execute_javascript with \
             document.execCommand('insertText', ...), or type_keystrokes."
        )
    }
}

/// The cross-platform `page` MCP tool.  Holds a backend trait object the
/// host platform constructs and passes in at registration time.
pub struct PageTool {
    backend: Arc<dyn PageBackend>,
    legacy_mutations_enabled: bool,
}

impl PageTool {
    pub fn new(backend: Arc<dyn PageBackend>) -> Self {
        Self {
            backend,
            legacy_mutations_enabled: env_flag_enabled(LEGACY_PAGE_MUTATIONS_ENV),
        }
    }

    #[cfg(test)]
    fn with_legacy_mutations(backend: Arc<dyn PageBackend>, enabled: bool) -> Self {
        Self {
            backend,
            legacy_mutations_enabled: enabled,
        }
    }
}

fn def() -> &'static ToolDef {
    static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
    DEF.get_or_init(|| ToolDef {
        name: "page".into(),
        description: "Legacy browser compatibility tool. Prefer get_browser_state and the \
            typed browser_* tools for exact targeting, endpoint ownership, and consent. \
            Read-only get_text and query_dom remain available by default. Mutating actions \
            require the daemon operator to set CUA_DRIVER_ENABLE_LEGACY_PAGE_MUTATIONS=1; \
            this escape hatch does not provide the typed browser surface's exact binding or \
            existing-profile grant guarantees. Supports \
            Chrome, Brave, Edge, Safari (via AppleScript on macOS), Electron apps (via CDP), \
            Chromium/Firefox on Windows (via UIA for read; CDP for execute_javascript when \
            --remote-debugging-port is set), and WKWebView/Tauri/AT-SPI fallbacks.\n\n\
            Actions:\n\
            - execute_javascript: Run JS and return the result.\n\
            - get_text: Extract visible text from the page.\n\
            - query_dom: Find elements matching a CSS selector.\n\
            - click_element: Click a CSS-selected element AND animate the agent cursor \
              to its on-screen center first (so the user sees what the agent is doing). \
              Prefer over `execute_javascript('el.click()')` whenever you want visible \
              cursor feedback.\n\
            - insert_text: Insert `text` at whatever currently holds DOM focus in one \
              native operation (CDP Input.insertText) — no synthesized key events, but \
              more durable than a one-shot execute_javascript write since rich-text \
              editors already have to treat it like an IME commit. Try this before \
              type_keystrokes on a contenteditable that discarded an execute_javascript \
              write. Click/focus the target field first.\n\
            - type_keystrokes: Type `text` via real per-character keystroke events into \
              whatever currently holds DOM focus. Slower than insert_text but the most \
              durable rung — use it when insert_text also gets discarded, or the editor's \
              own keydown/keyup handlers need to see real keys. Click/focus the target \
              field first.\n\
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
                    "enum": ["execute_javascript", "get_text", "query_dom", "click_element", "insert_text", "type_keystrokes", "enable_javascript_apple_events"],
                    "description": "Action to perform."
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector for click_element (e.g. 'button.submit', '#login a')."
                },
                "text": {
                    "type": "string",
                    "description": "Text to insert or type. Required for insert_text and type_keystrokes. The target field must already have DOM focus (click/focus it first)."
                },
                "cdp_port": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 65535,
                    "description": "Optional, for execute_javascript/insert_text/type_keystrokes: use this exact CDP port instead of auto-discovering one from pid. Needed when the port was opened via the browser's own remote-debugging toggle rather than a launch-time flag, since that path may not answer the auto-discovery probe."
                },
                "target_url_contains": {
                    "type": "string",
                    "description": "Optional, for execute_javascript/insert_text/type_keystrokes: require exactly one browser tab whose URL contains this substring. Use this on a multi-tab browser — there's no built-in link between window_id and which tab a CDP call reaches."
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

        if is_mutating_action(&action) && !self.legacy_mutations_enabled {
            return ToolResult::error(format!(
                "legacy page mutation {action:?} is disabled by default because it does not use \
                 the typed browser surface's exact endpoint and consent checks; use \
                 get_browser_state plus browser_* tools, or have the daemon operator explicitly \
                 set {LEGACY_PAGE_MUTATIONS_ENV}=1 for temporary compatibility"
            ));
        }

        // `pid` / `window_id` are resolved per-action: every action except
        // `enable_javascript_apple_events` needs both. We resolve once here
        // so each arm can `?` on the Result and we get matching error text.
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

        let (pid, window_id) = if action == "enable_javascript_apple_events" {
            (0i32, 0u64) // unused
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
                let cdp_port = match optional_cdp_port(&args) {
                    Ok(value) => value,
                    Err(error) => return ToolResult::error(error),
                };
                let target_url_contains = args.get("target_url_contains").and_then(|v| v.as_str());
                match self
                    .backend
                    .execute_javascript_targeted(pid, window_id, &js, cdp_port, target_url_contains)
                    .await
                {
                    Ok(result) => ToolResult::text(result),
                    Err(e) => ToolResult::error(format!("{e}")),
                }
            }

            "get_text" => match self.backend.get_text(pid, window_id).await {
                Ok(text) => ToolResult::text(text),
                Err(e) => ToolResult::error(format!("Page text extraction failed: {e}")),
            },

            "click_element" => {
                // Trim before emptiness check so whitespace-only selectors
                // fail fast at the input boundary rather than blowing up
                // inside the backend's JS payload (`querySelector("   ")`
                // returns null and the error there is much less specific).
                let selector =
                    match args.get("selector").and_then(|v| v.as_str()).map(str::trim) {
                        Some(s) if !s.is_empty() => s.to_owned(),
                        _ => return ToolResult::error(
                            "Missing required parameter: selector (CSS selector for click_element)",
                        ),
                    };
                match self.backend.click_element(pid, window_id, &selector).await {
                    Ok(res) => {
                        let structured = serde_json::json!({
                            "screen_x":   res.screen_x,
                            "screen_y":   res.screen_y,
                            "viewport_x": res.viewport_x,
                            "viewport_y": res.viewport_y,
                            "selector":   selector,
                        });
                        ToolResult::text(res.message).with_structured(structured)
                    }
                    Err(e) => ToolResult::error(format!("click_element failed: {e}")),
                }
            }

            "insert_text" => {
                let text = match args.get("text").and_then(|v| v.as_str()) {
                    Some(v) => v.to_owned(),
                    None => return ToolResult::error("Missing required parameter: text"),
                };
                let cdp_port = match optional_cdp_port(&args) {
                    Ok(value) => value,
                    Err(error) => return ToolResult::error(error),
                };
                let target_url_contains = args.get("target_url_contains").and_then(|v| v.as_str());
                match self
                    .backend
                    .insert_text(pid, window_id, &text, cdp_port, target_url_contains)
                    .await
                {
                    Ok(msg) => ToolResult::text(msg),
                    Err(e) => ToolResult::error(format!("insert_text failed: {e}")),
                }
            }

            "type_keystrokes" => {
                let text = match args.get("text").and_then(|v| v.as_str()) {
                    Some(v) => v.to_owned(),
                    None => return ToolResult::error("Missing required parameter: text"),
                };
                let cdp_port = match optional_cdp_port(&args) {
                    Ok(value) => value,
                    Err(error) => return ToolResult::error(error),
                };
                let target_url_contains = args.get("target_url_contains").and_then(|v| v.as_str());
                match self
                    .backend
                    .type_keystrokes(pid, window_id, &text, cdp_port, target_url_contains)
                    .await
                {
                    Ok(msg) => ToolResult::text(msg),
                    Err(e) => ToolResult::error(format!("type_keystrokes failed: {e}")),
                }
            }

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

fn optional_cdp_port(args: &Value) -> Result<Option<u16>, String> {
    let Some(value) = args.get("cdp_port") else {
        return Ok(None);
    };
    let raw = value.as_u64().ok_or_else(|| {
        "Invalid parameter: cdp_port must be an integer from 1 to 65535".to_owned()
    })?;
    let port = u16::try_from(raw)
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| format!("Invalid parameter: cdp_port {raw} outside 1..=65535"))?;
    Ok(Some(port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type TargetedCall = (i32, u64, String, Option<u16>, Option<String>);

    #[derive(Default)]
    struct RecordingBackend {
        targeted: Mutex<Option<TargetedCall>>,
    }

    #[async_trait]
    impl PageBackend for RecordingBackend {
        async fn get_text(&self, _pid: i32, _window_id: u64) -> anyhow::Result<String> {
            Ok(String::new())
        }

        async fn query_dom(
            &self,
            _pid: i32,
            _window_id: u64,
            _css_selector: &str,
            _attributes: &[String],
        ) -> anyhow::Result<String> {
            Ok(String::new())
        }

        async fn execute_javascript(
            &self,
            _pid: i32,
            _window_id: u64,
            _javascript: &str,
        ) -> anyhow::Result<String> {
            anyhow::bail!("untargeted execute must not be used")
        }

        async fn execute_javascript_targeted(
            &self,
            pid: i32,
            window_id: u64,
            javascript: &str,
            cdp_port: Option<u16>,
            target_url_contains: Option<&str>,
        ) -> anyhow::Result<String> {
            *self.targeted.lock().unwrap() = Some((
                pid,
                window_id,
                javascript.to_owned(),
                cdp_port,
                target_url_contains.map(str::to_owned),
            ));
            Ok("targeted".to_owned())
        }
    }

    #[tokio::test]
    async fn execute_javascript_forwards_explicit_page_target() {
        let backend = Arc::new(RecordingBackend::default());
        let tool = PageTool::with_legacy_mutations(backend.clone(), true);
        let synthetic_wayland_window_id = u64::from(u32::MAX) + 0x1234;

        let result = tool
            .invoke(serde_json::json!({
                "pid": 42,
                "window_id": synthetic_wayland_window_id,
                "action": "execute_javascript",
                "javascript": "document.title",
                "cdp_port": 9333,
                "target_url_contains": "#window-b"
            }))
            .await;

        assert!(result.is_error.is_none());
        assert_eq!(
            *backend.targeted.lock().unwrap(),
            Some((
                42,
                synthetic_wayland_window_id,
                "document.title".to_owned(),
                Some(9333),
                Some("#window-b".to_owned()),
            ))
        );
    }

    #[test]
    fn schema_advertises_targeted_execute_javascript() {
        let schema = &def().input_schema["properties"];
        assert_eq!(schema["cdp_port"]["minimum"], 1);
        assert_eq!(schema["cdp_port"]["maximum"], 65535);
        assert!(schema["cdp_port"]["description"]
            .as_str()
            .unwrap()
            .contains("execute_javascript"));
        assert!(schema["target_url_contains"]["description"]
            .as_str()
            .unwrap()
            .contains("exactly one"));
    }

    #[tokio::test]
    async fn every_legacy_mutation_refuses_without_operator_opt_in() {
        let backend = Arc::new(RecordingBackend::default());
        let tool = PageTool::with_legacy_mutations(backend.clone(), false);

        for action in [
            "execute_javascript",
            "click_element",
            "insert_text",
            "type_keystrokes",
            "enable_javascript_apple_events",
        ] {
            let result = tool.invoke(serde_json::json!({ "action": action })).await;
            assert_eq!(result.is_error, Some(true), "{action}");
            assert!(
                serde_json::to_string(&result)
                    .unwrap()
                    .contains(LEGACY_PAGE_MUTATIONS_ENV),
                "{action}"
            );
        }
        assert!(backend.targeted.lock().unwrap().is_none());
        assert!(def().open_world);
    }

    #[tokio::test]
    async fn rejects_out_of_range_cdp_port_before_dispatch() {
        let backend = Arc::new(RecordingBackend::default());
        let tool = PageTool::with_legacy_mutations(backend.clone(), true);

        let result = tool
            .invoke(serde_json::json!({
                "pid": 42,
                "window_id": 7,
                "action": "execute_javascript",
                "javascript": "document.title",
                "cdp_port": 65536
            }))
            .await;

        assert_eq!(result.is_error, Some(true));
        assert!(backend.targeted.lock().unwrap().is_none());
    }
}
