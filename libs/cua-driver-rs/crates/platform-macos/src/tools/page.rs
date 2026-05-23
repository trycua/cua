//! macOS implementation of the cross-platform `PageBackend` trait.
//!
//! Routes by `bundle_id` / Electron-detection / WKWebView-detection to:
//!   - **Apple Events** (Chromium-family + Safari) — zero-config, uses
//!     `osascript do JavaScript`.
//!   - **CDP** (Electron) — port discovered from the running process.
//!   - **AX-tree fallback** (WKWebView / Tauri) — read-only, no JS.
//!
//! The MCP tool definition (name, schema, action dispatch) lives in
//! `mcp_server::page` — this file only implements the backend.

use async_trait::async_trait;
use mcp_server::page::PageBackend;
use std::sync::Arc;

use super::ToolState;
use crate::browser::{is_wk_web_view_app, AXPageReader, BrowserJs, ElectronJs};

pub struct MacOsPageBackend {
    #[allow(dead_code)] // reserved for future per-pid caches / config
    pub state: Arc<ToolState>,
}

impl MacOsPageBackend {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }

    /// Resolve `bundle_id` for `pid` via the running-apps list.
    async fn bundle_id_for(pid: i32) -> String {
        tokio::task::spawn_blocking(move || {
            crate::apps::list_running_apps()
                .into_iter()
                .find(|a| a.pid == pid)
                .and_then(|a| a.bundle_id)
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    }
}

#[async_trait]
impl PageBackend for MacOsPageBackend {
    async fn get_text(&self, pid: i32, window_id: u32) -> anyhow::Result<String> {
        let bundle_id = Self::bundle_id_for(pid).await;

        let use_ax_fallback = !BrowserJs::supports(&bundle_id)
            && tokio::task::spawn_blocking(move || is_wk_web_view_app(pid))
                .await
                .unwrap_or(false);

        if use_ax_fallback {
            return ax_text_fallback(pid, window_id).await;
        }

        // Try JS path first; on error, fall back to AX walk.
        match execute_js("document.body.innerText", &bundle_id, pid, window_id).await {
            Ok(result) => Ok(result),
            Err(_) => ax_text_fallback(pid, window_id).await,
        }
    }

    async fn query_dom(
        &self,
        pid: i32,
        window_id: u32,
        css_selector: &str,
        attributes: &[String],
    ) -> anyhow::Result<String> {
        let bundle_id = Self::bundle_id_for(pid).await;

        let use_ax_fallback = !BrowserJs::supports(&bundle_id)
            && tokio::task::spawn_blocking(move || is_wk_web_view_app(pid))
                .await
                .unwrap_or(false);

        if use_ax_fallback {
            let results = ax_query_fallback(pid, window_id, css_selector).await?;
            return Ok(format_ax_elements(&results));
        }

        let js = build_query_selector_js(css_selector, attributes);
        match execute_js(&js, &bundle_id, pid, window_id).await {
            Ok(result) => Ok(result),
            Err(_) => {
                let results = ax_query_fallback(pid, window_id, css_selector).await?;
                Ok(format_ax_elements(&results))
            }
        }
    }

    async fn execute_javascript(
        &self,
        pid: i32,
        window_id: u32,
        javascript: &str,
    ) -> anyhow::Result<String> {
        let bundle_id = Self::bundle_id_for(pid).await;
        execute_js(javascript, &bundle_id, pid, window_id).await
    }

    async fn enable_javascript_apple_events(&self, bundle_id: &str) -> anyhow::Result<String> {
        BrowserJs::enable_javascript_apple_events(bundle_id).await?;
        Ok(
            "JavaScript from Apple Events has been enabled. The browser is restarting."
                .to_owned(),
        )
    }
}

/// Route JavaScript execution to the appropriate backend.
async fn execute_js(
    js: &str,
    bundle_id: &str,
    pid: i32,
    window_id: u32,
) -> anyhow::Result<String> {
    if BrowserJs::supports(bundle_id) {
        return BrowserJs::execute(js, bundle_id, window_id).await;
    }
    let is_electron = tokio::task::spawn_blocking(move || ElectronJs::is_electron(pid)).await?;
    if is_electron {
        return ElectronJs::execute(js, pid).await;
    }
    let is_wk = tokio::task::spawn_blocking(move || is_wk_web_view_app(pid)).await?;
    if is_wk {
        anyhow::bail!(
            "execute_javascript is not available for WKWebView/Tauri apps. \
             Use get_text or query_dom instead."
        );
    }
    anyhow::bail!("Unsupported browser: bundle_id={bundle_id}");
}

/// Extract page text via the AX tree.
async fn ax_text_fallback(pid: i32, window_id: u32) -> anyhow::Result<String> {
    let result = tokio::task::spawn_blocking(move || {
        crate::ax::tree::walk_tree(pid, Some(window_id), None)
    })
    .await
    .map_err(|e| anyhow::anyhow!("AX walk task failed: {e}"))?;
    Ok(AXPageReader::extract_text(&result.tree_markdown))
}

/// Query AX tree by CSS selector.
async fn ax_query_fallback(
    pid: i32,
    window_id: u32,
    selector: &str,
) -> anyhow::Result<Vec<crate::browser::ax_page_reader::AXElement>> {
    let sel = selector.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        crate::ax::tree::walk_tree(pid, Some(window_id), None)
    })
    .await
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
        let title = if !el.title.is_empty() {
            format!(" \"{}\"", el.title)
        } else {
            String::new()
        };
        let value = if !el.value.is_empty() {
            format!(" = \"{}\"", el.value)
        } else {
            String::new()
        };
        let desc = if !el.description.is_empty() {
            format!(" ({})", el.description)
        } else {
            String::new()
        };
        lines.push(format!("- {index_str}{}{title}{value}{desc}", el.role));
    }
    lines.join("\n")
}

/// Build a querySelectorAll JS snippet.
fn build_query_selector_js(selector: &str, attributes: &[String]) -> String {
    let escaped_sel = json_string(selector);
    let attrs_js = if attributes.is_empty() {
        "['tagName','textContent','href','id','class','type','value','placeholder','name']"
            .to_owned()
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
