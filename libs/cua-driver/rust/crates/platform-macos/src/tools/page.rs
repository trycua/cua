// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! macOS implementation of the cross-platform `PageBackend` trait.
//!
//! Routes by `bundle_id` / Electron-detection / WKWebView-detection to:
//!   - **Apple Events** (Chromium-family + Safari) — zero-config, uses
//!     `osascript do JavaScript`.
//!   - **CDP** (Electron) — port discovered from the running process.
//!   - **AX-tree fallback** (WKWebView / Tauri) — read-only, no JS.
//!
//! The MCP tool definition (name, schema, action dispatch) lives in
//! `cua_driver_core::page` — this file only implements the backend.

use async_trait::async_trait;
use cua_driver_core::page::{PageBackend, PageReadResult};
use std::{future::Future, sync::Arc};

use super::ToolState;
use crate::browser::{is_wk_web_view_app, AXPageReader, BrowserJs, CdpClient, ElectronJs};

pub struct MacOsPageBackend {
    pub state: Arc<ToolState>,
}

impl MacOsPageBackend {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }

    /// Evaluate `js` through a CDP session the driver already holds for this
    /// browser. Never opens a new connection: a new one makes Chrome show its
    /// "Allow remote debugging?" prompt and pull itself to the front.
    async fn attached_cdp_evaluate(&self, pid: i32, js: &str) -> Option<String> {
        let port = CdpClient::find_port_for_pid(pid).await?;
        if !self.state.cdp_sessions.has_session(port).await {
            return None;
        }
        self.state.cdp_sessions.evaluate(js, port, None).await.ok()
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
    async fn get_text(&self, pid: i32, window_id: u64) -> anyhow::Result<PageReadResult> {
        let bundle_id = Self::bundle_id_for(pid).await;
        read_page(
            self,
            &bundle_id,
            pid,
            window_id,
            "document.body.innerText",
            || ax_text_fallback(pid, window_id, BrowserJs::supports(&bundle_id)),
        )
        .await
    }

    async fn query_dom(
        &self,
        pid: i32,
        window_id: u64,
        css_selector: &str,
        attributes: &[String],
    ) -> anyhow::Result<PageReadResult> {
        let bundle_id = Self::bundle_id_for(pid).await;
        let js = build_query_selector_js(css_selector, attributes);
        read_page(self, &bundle_id, pid, window_id, &js, || {
            ax_query_fallback(
                pid,
                window_id,
                css_selector,
                BrowserJs::supports(&bundle_id),
            )
        })
        .await
    }
}

/// Route JavaScript execution to the appropriate backend.
async fn execute_js(js: &str, bundle_id: &str, pid: i32, window_id: u64) -> anyhow::Result<String> {
    if BrowserJs::supports(bundle_id) {
        let window_id = u32::try_from(window_id)
            .map_err(|_| anyhow::anyhow!("macOS window_id {window_id} is out of u32 range"))?;
        return BrowserJs::execute(js, bundle_id, pid, window_id).await;
    }
    let is_electron = tokio::task::spawn_blocking(move || ElectronJs::is_electron(pid)).await?;
    if is_electron {
        return ElectronJs::execute(js, pid).await;
    }
    let is_wk = tokio::task::spawn_blocking(move || is_wk_web_view_app(pid)).await?;
    if is_wk {
        anyhow::bail!(
            "JavaScript is not available for WKWebView/Tauri apps. \
             Read the page through its accessibility tree instead."
        );
    }
    anyhow::bail!("Unsupported browser: bundle_id={bundle_id}");
}

/// The tree rows that are page content. A browser window's walk also holds
/// the app's menu bar and the browser toolbar, so a browser read is scoped to
/// the window's `AXWebArea` and fails when there is none, instead of
/// returning the menu bar as if it were the page. Other web-view apps keep
/// the whole tree when they expose no web area.
fn page_rows(tree_markdown: String, require_web_area: bool) -> anyhow::Result<String> {
    match AXPageReader::web_area_markdown(&tree_markdown) {
        Some(page) => Ok(page),
        None if require_web_area => anyhow::bail!(
            "This window's page content is not readable: its accessibility tree has no \
             AXWebArea (only the browser's own controls and the menu bar). Read it again in \
             a moment, use get_browser_state, or attach CDP with browser_prepare."
        ),
        None => Ok(tree_markdown),
    }
}

async fn read_page<F, A>(
    backend: &MacOsPageBackend,
    bundle_id: &str,
    pid: i32,
    window_id: u64,
    javascript: &str,
    ax_fallback: F,
) -> anyhow::Result<PageReadResult>
where
    F: FnOnce() -> A,
    A: Future<Output = anyhow::Result<(String, bool)>>,
{
    let use_ax = !BrowserJs::supports(bundle_id)
        && tokio::task::spawn_blocking(move || is_wk_web_view_app(pid))
            .await
            .unwrap_or(false);
    let mut source = if BrowserJs::supports(bundle_id) {
        "javascript"
    } else {
        "cdp"
    };
    let primary = if use_ax {
        None
    } else {
        Some(execute_js(javascript, bundle_id, pid, window_id).await)
    };
    let mut error = None;
    let content = match primary {
        Some(Ok(content)) => Some(content),
        Some(Err(failure)) => {
            error = Some(failure.to_string());
            source = "cdp";
            backend.attached_cdp_evaluate(pid, javascript).await
        }
        None => None,
    };
    let fallback = if content.is_none() {
        ax_fallback().await
    } else {
        Ok((String::new(), false))
    };
    finish_page_read(source, content, error, fallback)
}

fn finish_page_read(
    mut source: &str,
    content: Option<String>,
    error: Option<String>,
    fallback: anyhow::Result<(String, bool)>,
) -> anyhow::Result<PageReadResult> {
    let (content, truncated) = match content {
        Some(content) => (content, None),
        None => {
            let (content, truncated) = fallback.map_err(|ax| match error.as_ref() {
                Some(error) => anyhow::anyhow!("{ax} (JavaScript read failed first: {error})"),
                None => ax,
            })?;
            source = "ax_fallback";
            (content, Some(truncated))
        }
    };
    let mut metadata = serde_json::json!({"source": source});
    if let Some(truncated) = truncated {
        if let Some(error) = error {
            metadata["fallback_error"] = serde_json::json!(error);
        }
        metadata["truncated"] = serde_json::json!(truncated);
    }
    Ok(PageReadResult::text(content).with_structured(metadata))
}

/// Extract page text via the bounded AX tree.
async fn ax_text_fallback(
    pid: i32,
    window_id: u64,
    require_web_area: bool,
) -> anyhow::Result<(String, bool)> {
    let window_id = u32::try_from(window_id)
        .map_err(|_| anyhow::anyhow!("macOS window_id {window_id} is out of u32 range"))?;
    let result =
        tokio::task::spawn_blocking(move || crate::ax::tree::walk_tree(pid, Some(window_id), None))
            .await
            .map_err(|e| anyhow::anyhow!("AX walk task failed: {e}"))?;
    Ok((
        AXPageReader::extract_text(&page_rows(result.tree_markdown, require_web_area)?),
        result.truncated || result.depth_truncated,
    ))
}

/// Query the bounded AX tree by CSS selector.
async fn ax_query_fallback(
    pid: i32,
    window_id: u64,
    selector: &str,
    require_web_area: bool,
) -> anyhow::Result<(String, bool)> {
    let window_id = u32::try_from(window_id)
        .map_err(|_| anyhow::anyhow!("macOS window_id {window_id} is out of u32 range"))?;
    let sel = selector.to_owned();
    let result =
        tokio::task::spawn_blocking(move || crate::ax::tree::walk_tree(pid, Some(window_id), None))
            .await
            .map_err(|e| anyhow::anyhow!("AX walk task failed: {e}"))?;
    Ok((
        format_ax_elements(&AXPageReader::query(
            &sel,
            &page_rows(result.tree_markdown, require_web_area)?,
        )),
        result.truncated || result.depth_truncated,
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    const MENU_ONLY: &str = "- [0] AXMenuBar\n  - [1] AXMenuBarItem \"Apple\"\n";

    #[test]
    fn browser_read_without_a_web_area_fails_instead_of_returning_the_menu_bar() {
        let error = page_rows(MENU_ONLY.to_owned(), true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("AXWebArea"), "{error}");
    }

    #[test]
    fn browser_read_is_scoped_to_the_web_area() {
        let tree = format!("{MENU_ONLY}- [2] AXWindow \"w\"\n  - [3] AXWebArea \"p\"\n    - [4] AXStaticText \"hello\"\n");
        let page = page_rows(tree, true).unwrap();
        assert!(page.contains("hello"));
        assert!(!page.contains("Apple"));
    }

    #[test]
    fn web_view_apps_keep_the_whole_tree_without_a_web_area() {
        assert_eq!(page_rows(MENU_ONLY.to_owned(), false).unwrap(), MENU_ONLY);
    }

    #[test]
    fn page_read_decisions_preserve_provenance() {
        for source in ["javascript", "cdp"] {
            let page = finish_page_read(
                source,
                Some("page".into()),
                Some("JS failed".into()),
                Ok(("ax".into(), true)),
            )
            .unwrap();
            assert_eq!(
                page.structured_content.unwrap(),
                serde_json::json!({"source": source})
            );
        }
        for error in [None, Some("JS failed".to_owned())] {
            let page =
                finish_page_read("cdp", None, error.clone(), Ok(("ax".into(), true))).unwrap();
            let mut expected = serde_json::json!({"source": "ax_fallback", "truncated": true});
            if let Some(error) = error.as_ref() {
                expected["fallback_error"] = serde_json::json!(error);
            }
            assert_eq!(page.structured_content.unwrap(), expected);
            let failure = finish_page_read(
                "cdp",
                None,
                error.clone(),
                Err(anyhow::anyhow!("AX failed")),
            )
            .unwrap_err()
            .to_string();
            assert_eq!(
                failure,
                match error {
                    Some(error) => format!("AX failed (JavaScript read failed first: {error})"),
                    None => "AX failed".to_owned(),
                }
            );
        }
    }
}
