//! Linux implementation of the cross-platform `PageBackend` trait.
//!
//! Read paths use AT-SPI via the existing `walk_tree` infrastructure
//! (`crate::atspi`). `execute_javascript` calls the shared
//! `mcp_server::cdp` helper — Chromium/Electron must be launched with
//! `--remote-debugging-port=N` for that to succeed.

use async_trait::async_trait;
use mcp_server::page::PageBackend;

pub struct LinuxPageBackend;

impl LinuxPageBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxPageBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PageBackend for LinuxPageBackend {
    async fn get_text(&self, pid: i32, window_id: u32) -> anyhow::Result<String> {
        let pid_u = pid as u32;
        let xid = window_id as u64;
        let result = tokio::task::spawn_blocking(move || {
            crate::atspi::walk_tree(pid_u, xid, None)
        })
        .await
        .map_err(|e| anyhow::anyhow!("AT-SPI walk task failed: {e}"))?;
        Ok(extract_text_from_markdown(&result.tree_markdown))
    }

    async fn query_dom(
        &self,
        pid: i32,
        window_id: u32,
        css_selector: &str,
        _attributes: &[String],
    ) -> anyhow::Result<String> {
        let pid_u = pid as u32;
        let xid = window_id as u64;
        let selector = css_selector.to_owned();
        let result = tokio::task::spawn_blocking(move || {
            crate::atspi::walk_tree(pid_u, xid, None)
        })
        .await
        .map_err(|e| anyhow::anyhow!("AT-SPI walk task failed: {e}"))?;

        if selector.contains("[data-") {
            anyhow::bail!(
                "`[data-*]` selectors are not reachable via AT-SPI on Linux. \
                 Use `execute_javascript` for full DOM access (requires the browser \
                 launched with `--remote-debugging-port`)."
            );
        }

        let role = role_for_selector(&selector);
        let selector_is_wildcard = {
            let s = selector.trim();
            s.is_empty() || s == "*"
        };
        // `role_for_selector` collapses both "wildcard / match-all" AND
        // "unrecognised" into `None`. The first should keep every node; the
        // second should fail loudly so an agent doesn't get a misleading
        // dump of every element on the page when its `.foo` / `#bar` query
        // wasn't actually understood.
        if role.is_none() && !selector_is_wildcard {
            anyhow::bail!(
                "Selector '{selector}' is not supported by Linux AT-SPI role mapping. \
                 Use a simple tag selector (a, button, input, textarea, h1-h6, img, li, \
                 p, select, *), or `execute_javascript` for full DOM access (requires the \
                 browser launched with `--remote-debugging-port`)."
            );
        }
        let lines: Vec<String> = result
            .nodes
            .iter()
            .filter(|n| match role.as_deref() {
                Some(r) => n.role.eq_ignore_ascii_case(r),
                None => true, // safe now — only reached when selector is wildcard
            })
            .map(|n| {
                let name = n.name.as_deref().unwrap_or("");
                let value = n.value.as_deref().unwrap_or("");
                let index = n
                    .element_index
                    .map(|i| format!("[{i}] "))
                    .unwrap_or_default();
                if value.is_empty() {
                    format!("- {index}{} \"{}\"", n.role, name)
                } else {
                    format!("- {index}{} \"{}\" = \"{}\"", n.role, name, value)
                }
            })
            .collect();

        if lines.is_empty() {
            Ok("No elements found.".to_owned())
        } else {
            Ok(lines.join("\n"))
        }
    }

    async fn execute_javascript(
        &self,
        _pid: i32,
        _window_id: u32,
        javascript: &str,
    ) -> anyhow::Result<String> {
        let port: u16 = match std::env::var("CUA_DRIVER_CDP_PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
        {
            Some(p) => p,
            None => anyhow::bail!(
                "Chromium's `execute_javascript` on Linux requires the browser launched \
                 with `--remote-debugging-port=N`. Relaunch the browser with the flag and \
                 export CUA_DRIVER_CDP_PORT=N before starting cua-driver, or use \
                 `get_text` / `query_dom` for read-only access."
            ),
        };
        let result = mcp_server::cdp::evaluate(port, javascript, true).await?;
        Ok(format!("cdp.runtime.evaluate.user_gesture: {result}"))
    }
}

/// Map common CSS selectors to AT-SPI role names. Returns `None` for `*` or
/// unrecognised selectors (caller treats `None` as "no role filter").
fn role_for_selector(sel: &str) -> Option<String> {
    let s = sel.trim();
    // Trim trailing [attr=...] or .class / #id qualifiers — we filter on
    // role only here (the macOS backend has richer JS-side filtering).
    let tag_end = s
        .find(|c: char| c == '[' || c == '.' || c == '#')
        .unwrap_or(s.len());
    let tag = &s[..tag_end];
    let role = match tag.to_ascii_lowercase().as_str() {
        "a" => "link",
        "button" => "push button",
        "input" | "textarea" => "entry",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading",
        "img" => "image",
        "li" => "list item",
        "p" => "paragraph",
        "select" => "combo box",
        "" | "*" => return None,
        _ => return None,
    };
    Some(role.to_owned())
}

/// Strip Markdown tree formatting and concatenate the human-readable text.
fn extract_text_from_markdown(md: &str) -> String {
    md.lines()
        .map(|l| {
            // Strip leading indent + bullet markers `-` / `[N]` so the result
            // reads like flowing text.
            let trimmed = l.trim_start();
            let trimmed = trimmed.trim_start_matches('-').trim_start();
            // Drop the `[N] ` element-index prefix.
            if let Some(rest) = trimmed.strip_prefix('[') {
                if let Some(end) = rest.find(']') {
                    return rest[end + 1..].trim_start().to_owned();
                }
            }
            trimmed.to_owned()
        })
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
