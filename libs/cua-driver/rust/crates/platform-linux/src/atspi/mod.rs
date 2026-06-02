//! AT-SPI accessibility tree walking for Linux.
//!
//! AT-SPI2 is exposed over D-Bus. We talk to it natively in Rust via the
//! `atspi` crate (zbus) — no Python, `pyatspi`, or GObject-introspection
//! typelibs are required at runtime. The async zbus calls run on a shared
//! background Tokio runtime; the public functions stay synchronous because
//! callers invoke them inside `tokio::task::spawn_blocking`.
//!
//! When the AT-SPI bus is unavailable (or the app exposes no a11y tree) we
//! fall back to a minimal X11 property tree (window title + role) via x11rb.

use anyhow::Result;

pub mod cache;
pub mod native;
pub use cache::ElementCache;

#[derive(Clone, Debug)]
pub struct AtspiNode {
    pub element_index: Option<usize>,
    pub role: String,
    pub name: Option<String>,
    pub value: Option<String>,
    pub description: Option<String>,
    pub actions: Vec<String>,
    /// For pyatspi path: element_key = element_index as u64.
    /// For X11 fallback: element_key = xid.
    pub element_key: u64,
}

pub struct AtspiTreeResult {
    pub tree_markdown: String,
    pub nodes: Vec<AtspiNode>,
}

/// Walk the AT-SPI tree for a window identified by (pid, xid).
/// Falls back to a minimal X11 property tree if AT-SPI is unavailable.
pub fn walk_tree(pid: u32, xid: u64, query: Option<&str>) -> AtspiTreeResult {
    // Native AT-SPI (most complete).
    if let Ok(Some((raw_md, nodes))) = native::walk_tree(pid) {
        if !raw_md.is_empty() {
            let md = if let Some(q) = query { filter_tree(&raw_md, q) } else { raw_md };
            return AtspiTreeResult { tree_markdown: md, nodes };
        }
    }

    // Fallback: X11 window properties as minimal tree.
    walk_via_x11_properties(xid, query)
}

/// Perform the first advertised action on element `idx` within pid's app tree.
/// Returns Ok(action_name) on success.
pub fn perform_action(pid: u32, idx: usize) -> Result<String> {
    native::perform_action(pid, idx)
}

/// Set the text value of element `idx` within pid's app tree via AT-SPI.
/// Tries `EditableText.set_text_contents(value)` first, then
/// `Value.set_current_value(float)`.
pub fn set_value(pid: u32, idx: usize, value: &str) -> Result<()> {
    native::set_value(pid, idx, value)
}

/// Insert `text` into a GUI app's editable field via AT-SPI EditableText —
/// focus-free and toolkit-agnostic, unlike X11 key injection which only reaches
/// the *focused* toplevel's focused widget. Targets the focused editable element
/// if the toolkit exposes one, else the first editable element in the tree.
/// Returns Ok(true) if text was inserted, Ok(false) if the app exposes no
/// editable element (so the caller can fall back), Err on an AT-SPI failure.
pub fn insert_text(pid: u32, text: &str) -> Result<bool> {
    native::insert_text(pid, text)
}

/// Get the screen-coordinate bounding box (x, y, width, height) of element `idx`.
pub fn get_element_bounds(pid: u32, idx: usize) -> Result<(i32, i32, u32, u32)> {
    native::get_element_bounds(pid, idx)
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Minimal X11 property-based tree (fallback when AT-SPI is unavailable).
fn walk_via_x11_properties(xid: u64, query: Option<&str>) -> AtspiTreeResult {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;

    let (conn, _) = match RustConnection::connect(None) {
        Ok(r) => r,
        Err(_) => return AtspiTreeResult { tree_markdown: String::new(), nodes: vec![] },
    };

    let window = xid as u32;

    // Read window title.
    let title = get_x11_title(&conn, window).unwrap_or_default();

    // Read WM_CLASS.
    let wm_class = get_x11_wm_class(&conn, window).unwrap_or_default();

    let mut md = String::new();
    let mut nodes = vec![];

    let root_node = AtspiNode {
        element_index: Some(0),
        role: "window".into(),
        name: if title.is_empty() { None } else { Some(title.clone()) },
        value: None,
        description: if wm_class.is_empty() { None } else { Some(wm_class.clone()) },
        actions: vec!["activate".into()],
        element_key: xid,
    };
    md.push_str(&format!("- [0] window \"{}\" [actions=[activate]]\n", title));
    nodes.push(root_node);

    let raw_md = md;
    let tree_markdown = if let Some(q) = query {
        filter_tree(&raw_md, q)
    } else {
        raw_md
    };

    AtspiTreeResult { tree_markdown, nodes }
}

fn get_x11_title(conn: &x11rb::rust_connection::RustConnection, window: u32) -> Option<String> {
    use x11rb::protocol::xproto::*;
    // Try _NET_WM_NAME first.
    let net_wm_name = conn.intern_atom(false, b"_NET_WM_NAME").ok()?.reply().ok()?.atom;
    let utf8_string = conn.intern_atom(false, b"UTF8_STRING").ok()?.reply().ok()?.atom;
    if let Ok(reply) = conn.get_property(false, window, net_wm_name, utf8_string, 0, 1024).ok()?.reply() {
        if !reply.value.is_empty() {
            return Some(String::from_utf8_lossy(&reply.value).into_owned());
        }
    }
    let reply = conn.get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024).ok()?.reply().ok()?;
    Some(String::from_utf8_lossy(&reply.value).into_owned())
}

fn get_x11_wm_class(conn: &x11rb::rust_connection::RustConnection, window: u32) -> Option<String> {
    use x11rb::protocol::xproto::*;
    let reply = conn.get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 512).ok()?.reply().ok()?;
    let s = String::from_utf8_lossy(&reply.value);
    // WM_CLASS is two NUL-separated strings: instance_name\0class_name\0
    Some(s.trim_end_matches('\0').replace('\0', "."))
}

fn filter_tree(markdown: &str, query: &str) -> String {
    let needle = query.to_lowercase();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut ancestors: Vec<&str> = Vec::new();
    let mut last_emitted: Vec<Option<&str>> = Vec::new();
    let mut output: Vec<&str> = Vec::new();

    for line in &lines {
        let depth = line.chars().take_while(|c| *c == ' ').count() / 2;
        while ancestors.len() <= depth { ancestors.push(""); last_emitted.push(None); }
        for d in (depth+1)..ancestors.len() { last_emitted[d] = None; }
        ancestors[depth] = line;
        if line.to_lowercase().contains(&needle) {
            for d in 0..depth {
                if ancestors[d].is_empty() { continue; }
                if last_emitted[d] == Some(ancestors[d]) { continue; }
                last_emitted[d] = Some(ancestors[d]);
                output.push(ancestors[d]);
            }
            last_emitted[depth] = Some(line);
            output.push(line);
        }
    }
    if output.is_empty() { return String::new(); }
    let mut r = output.join("\n"); r.push('\n'); r
}
