//! PageTool: MCP tool for browser JS execution, text extraction, and DOM querying.

use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;
use crate::browser::{BrowserJs, ElectronJs, is_wk_web_view_app, AXPageReader};

pub struct PageTool {
    pub state: Arc<ToolState>,
}

impl PageTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "page".into(),
        description: "Interact with the browser page loaded in a running app. Supports \
            Chrome, Brave, Edge, Safari (via AppleScript), Electron apps (via CDP), \
            and WKWebView/Tauri apps (via AX tree fallback).\n\n\
            Actions:\n\
            - execute_javascript: Run JS and return the result.\n\
            - get_text: Extract visible text from the page.\n\
            - query_dom: Find elements matching a CSS selector.\n\
            - enable_javascript_apple_events: Patch Preferences to allow JS from Apple Events \
              (Chrome/Brave/Edge only, requires user confirmation and browser restart).".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "window_id", "action"],
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
                    "description": "Bundle ID of the browser. Required for enable_javascript_apple_events."
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
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as i32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };
        let window_id = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v as u32,
            None => return ToolResult::error("Missing required parameter: window_id"),
        };
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(v) => v.to_owned(),
            None => return ToolResult::error("Missing required parameter: action"),
        };

        // Resolve bundle_id from pid.
        let bundle_id = {
            let p = pid;
            tokio::task::spawn_blocking(move || {
                crate::apps::list_running_apps()
                    .into_iter()
                    .find(|a| a.pid == p)
                    .and_then(|a| a.bundle_id)
                    .unwrap_or_default()
            }).await.unwrap_or_default()
        };

        match action.as_str() {
            "enable_javascript_apple_events" => {
                let confirmed = args.get("user_has_confirmed_enabling")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !confirmed {
                    return ToolResult::error(
                        "Set user_has_confirmed_enabling=true to proceed. \
                         This will quit and relaunch the browser."
                    );
                }
                let bid = args.get("bundle_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&bundle_id)
                    .to_owned();
                if bid.is_empty() {
                    return ToolResult::error("bundle_id is required for enable_javascript_apple_events");
                }
                match BrowserJs::enable_javascript_apple_events(&bid).await {
                    Ok(()) => ToolResult::text("JavaScript from Apple Events has been enabled. The browser is restarting."),
                    Err(e) => ToolResult::error(format!("Failed to enable JavaScript Apple Events: {e}")),
                }
            }

            "execute_javascript" => {
                let js = match args.get("javascript").and_then(|v| v.as_str()) {
                    Some(v) => v.to_owned(),
                    None => return ToolResult::error("Missing required parameter: javascript"),
                };
                match execute_js(&js, &bundle_id, pid, window_id).await {
                    Ok(result) => ToolResult::text(result),
                    Err(e) => ToolResult::error(format!("{e}")),
                }
            }

            "get_text" => {
                let use_ax_fallback = !BrowserJs::supports(&bundle_id) && {
                    let p = pid;
                    tokio::task::spawn_blocking(move || is_wk_web_view_app(p)).await.unwrap_or(false)
                };

                if use_ax_fallback {
                    match ax_text_fallback(pid, window_id).await {
                        Ok(text) => ToolResult::text(text),
                        Err(e) => ToolResult::error(format!("AX text extraction failed: {e}")),
                    }
                } else {
                    match execute_js("document.body.innerText", &bundle_id, pid, window_id).await {
                        Ok(result) => ToolResult::text(result),
                        Err(_) => {
                            // Try AX fallback on error.
                            match ax_text_fallback(pid, window_id).await {
                                Ok(text) => ToolResult::text(text),
                                Err(e) => ToolResult::error(format!("Page text extraction failed: {e}")),
                            }
                        }
                    }
                }
            }

            "query_dom" => {
                let selector = args.get("css_selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("*")
                    .to_owned();
                let attributes: Vec<String> = args.get("attributes")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default();

                let use_ax_fallback = !BrowserJs::supports(&bundle_id) && {
                    let p = pid;
                    tokio::task::spawn_blocking(move || is_wk_web_view_app(p)).await.unwrap_or(false)
                };

                if use_ax_fallback {
                    match ax_query_fallback(pid, window_id, &selector).await {
                        Ok(results) => ToolResult::text(format_ax_elements(&results)),
                        Err(e) => ToolResult::error(format!("AX DOM query failed: {e}")),
                    }
                } else {
                    let js = build_query_selector_js(&selector, &attributes);
                    match execute_js(&js, &bundle_id, pid, window_id).await {
                        Ok(result) => ToolResult::text(result),
                        Err(_) => {
                            // Try AX fallback.
                            match ax_query_fallback(pid, window_id, &selector).await {
                                Ok(results) => ToolResult::text(format_ax_elements(&results)),
                                Err(e) => ToolResult::error(format!("DOM query failed: {e}")),
                            }
                        }
                    }
                }
            }

            other => ToolResult::error(format!("Unknown action: {other}")),
        }
    }
}

/// Route JavaScript execution to the appropriate backend.
async fn execute_js(js: &str, bundle_id: &str, pid: i32, window_id: u32) -> anyhow::Result<String> {
    if BrowserJs::supports(bundle_id) {
        BrowserJs::execute(js, bundle_id, window_id).await
    } else {
        let p = pid;
        let is_electron = tokio::task::spawn_blocking(move || ElectronJs::is_electron(p)).await?;
        if is_electron {
            ElectronJs::execute(js, pid).await
        } else {
            let p = pid;
            let is_wk = tokio::task::spawn_blocking(move || is_wk_web_view_app(p)).await?;
            if is_wk {
                anyhow::bail!(
                    "execute_javascript is not available for WKWebView/Tauri apps. \
                     Use get_text or query_dom instead."
                );
            } else {
                anyhow::bail!("Unsupported browser: bundle_id={bundle_id}");
            }
        }
    }
}

/// Extract page text via the AX tree.
async fn ax_text_fallback(pid: i32, window_id: u32) -> anyhow::Result<String> {
    let result = tokio::task::spawn_blocking(move || {
        crate::ax::tree::walk_tree(pid, Some(window_id), None)
    }).await
    .map_err(|e| anyhow::anyhow!("AX walk task failed: {e}"))?;
    Ok(AXPageReader::extract_text(&result.tree_markdown))
}

/// Query AX tree by CSS selector.
async fn ax_query_fallback(pid: i32, window_id: u32, selector: &str) -> anyhow::Result<Vec<crate::browser::ax_page_reader::AXElement>> {
    let sel = selector.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        crate::ax::tree::walk_tree(pid, Some(window_id), None)
    }).await
    .map_err(|e| anyhow::anyhow!("AX walk task failed: {e}"))?;
    Ok(AXPageReader::query(&sel, &result.tree_markdown))
}

fn format_ax_elements(elements: &[crate::browser::ax_page_reader::AXElement]) -> String {
    if elements.is_empty() {
        return "No elements found.".to_owned();
    }
    let mut lines = Vec::new();
    for el in elements {
        let index_str = el.index.map(|i| format!("[{i}] ")).unwrap_or_default();
        let title = if !el.title.is_empty() { format!(" \"{}\"", el.title) } else { String::new() };
        let value = if !el.value.is_empty() { format!(" = \"{}\"", el.value) } else { String::new() };
        let desc = if !el.description.is_empty() { format!(" ({})", el.description) } else { String::new() };
        lines.push(format!("- {index_str}{}{title}{value}{desc}", el.role));
    }
    lines.join("\n")
}

/// Build a querySelectorAll JS snippet.
fn build_query_selector_js(selector: &str, attributes: &[String]) -> String {
    let escaped_sel = json_string(selector);
    let attrs_js = if attributes.is_empty() {
        "['tagName','textContent','href','id','class','type','value','placeholder','name']".to_owned()
    } else {
        let parts: Vec<String> = attributes.iter().map(|a| json_string(a)).collect();
        format!("[{}]", parts.join(","))
    };
    format!(
        r#"(function(){{
  var sel = {escaped_sel};
  var attrs = {attrs_js};
  var els = Array.from(document.querySelectorAll(sel));
  return JSON.stringify(els.map(function(el){{
    var obj = {{}};
    attrs.forEach(function(a){{ obj[a] = el[a] !== undefined ? el[a] : el.getAttribute(a); }});
    obj.textContent = (el.textContent||'').trim().substring(0,200);
    return obj;
  }}));
}})();"#
    )
}

fn json_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}
