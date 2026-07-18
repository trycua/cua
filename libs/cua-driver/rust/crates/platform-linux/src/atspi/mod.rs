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
pub use native::ensure_listener_active;

#[derive(Clone, Debug)]
pub struct AtspiNode {
    pub element_index: Option<usize>,
    pub role: String,
    pub name: Option<String>,
    pub value: Option<String>,
    /// Checked state when the accessibility backend exposes one for a toggle.
    pub checked: Option<bool>,
    pub description: Option<String>,
    pub actions: Vec<String>,
    /// For AT-SPI: element_key = element_index as u64.
    /// For X11 fallback: element_key = xid.
    pub element_key: u64,
    /// Depth in the markdown tree (0 = top-level window child).
    /// Defaults to 0 when not tracked (e.g. X11 fallback path).
    pub depth: usize,
    /// `element_index` of the nearest actionable ancestor, if any.
    /// Mirrors what the markdown indent shows.
    pub parent_element_index: Option<usize>,
    /// True when the native AT-SPI walker observed this node below renderer
    /// web content. Browser-owned consent UI must never match such nodes.
    pub in_web_content: bool,
}

pub struct AtspiTreeResult {
    pub tree_markdown: String,
    pub nodes: Vec<AtspiNode>,
    pub bounds: Vec<(usize, i32, i32, u32, u32)>,
}

/// Walk the AT-SPI tree for a window identified by (pid, xid).
/// Falls back to a minimal X11 property tree if AT-SPI is unavailable.
pub fn walk_tree(pid: u32, xid: u64, query: Option<&str>) -> AtspiTreeResult {
    walk_tree_bounded(pid, xid, query, None, None)
}

/// Walk the AT-SPI tree with caller-supplied caps. `None` for either cap
/// means "use the walker's built-in default" (5 000 nodes; unlimited depth).
/// These caps protect against Electron / large web apps that
/// produce 10k+ element trees and blow context windows.
pub fn walk_tree_bounded(
    pid: u32,
    xid: u64,
    query: Option<&str>,
    max_elements: Option<usize>,
    max_depth: Option<usize>,
) -> AtspiTreeResult {
    // Native AT-SPI (most complete). On a COLD launch the Qt6 (and some GTK)
    // AT-SPI bridge registers lazily — the first walk against a freshly
    // launched app can come back with just the root window (element_count=1,
    // no children) because `org.a11y.atspi.Registry` hasn't finished
    // enumerating the app's tree yet. Retry a few times with a short backoff
    // while the tree is suspiciously root-only, so the first get_window_state
    // after launch returns the real tree instead of an empty one. See #1927.
    const MAX_ATTEMPTS: usize = 4;
    for attempt in 0..MAX_ATTEMPTS {
        if let Ok(Some((raw_md, nodes, bounds))) =
            native::walk_tree_bounded(pid, xid, max_elements, max_depth)
        {
            // `nodes.len() <= 1` == only the root window resolved: the
            // cold-registry symptom. Accept any real tree immediately; only
            // keep waiting on the degenerate case, and accept it anyway on the
            // final attempt rather than discarding a (minimal) valid result.
            if !raw_md.is_empty() && (nodes.len() > 1 || attempt == MAX_ATTEMPTS - 1) {
                let md = if let Some(q) = query {
                    filter_tree(&raw_md, q)
                } else {
                    raw_md
                };
                return AtspiTreeResult {
                    tree_markdown: md,
                    nodes,
                    bounds,
                };
            }
        }
        if attempt < MAX_ATTEMPTS - 1 {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }

    // Fallback: X11 window properties as minimal tree.
    walk_via_x11_properties(xid, query)
}

/// Perform the first advertised action on element `idx` within pid's app tree.
/// Returns `Ok((action_name, suspected_noop))` on success — `suspected_noop`
/// is true when the actuated node looked like a silent no-op (a passive
/// display role, or no advertised action), so the caller can surface
/// `effect: "suspected_noop"`.
pub fn perform_action(pid: u32, idx: usize) -> Result<(String, bool)> {
    native::perform_action(pid, idx)
}

/// Give an indexed AT-SPI element keyboard focus without activating its window.
pub fn focus_element(pid: u32, idx: usize) -> Result<bool> {
    native::focus_element(pid, idx)
}

pub fn scroll_element(pid: u32, idx: usize, direction: &str, amount: usize) -> Result<()> {
    native::scroll_element(pid, idx, direction, amount)
}

/// Enumerate top-level windows from the AT-SPI registry. The window-listing
/// fallback for Wayland compositors without `zwlr_foreign_toplevel_management`
/// (GNOME Mutter / KDE KWin), where native apps have no X11 XID. Returns one
/// entry per application top-level frame with a synthetic, stable `xid` that
/// round-trips into the by-pid AT-SPI element flow. See [`native::list_windows`].
pub fn list_windows(filter_pid: Option<u32>) -> Vec<crate::x11::WindowInfo> {
    native::list_windows(filter_pid)
}

/// Resolve a window-local pixel to the actionable AT-SPI element at that point
/// and perform its primary action — the no-focus-steal way to land a *pixel*
/// click on toolkits (GTK) that drop synthetic X11 pointer events. Returns
/// `Ok(Some(action))` when an element was actuated, `Ok(None)` when no
/// actionable element covers the point (caller falls back to the X11 path).
pub fn perform_action_at_point(pid: u32, win_x: i32, win_y: i32) -> Result<Option<String>> {
    native::perform_action_at_point(pid, win_x, win_y)
}

/// Resolve a *screen* pixel to the indexable element whose reconstructed screen
/// frame covers it and fire its primary action by `element_index` — the
/// vision/pixel click that lands on Wayland (no pointer injection) and on GTK4
/// generally (no `CoordType::Screen`, which reports (0,0)). See
/// [`native::perform_action_at_screen_point`].
pub fn perform_action_at_screen_point(
    pid: u32,
    xid: u64,
    screen_x: i32,
    screen_y: i32,
) -> Result<Option<String>> {
    native::perform_action_at_screen_point(pid, xid, screen_x, screen_y)
}

/// Try to type text into any editable field in the window via AT-SPI EditableText.
/// This works for unfocused windows if the toolkit exposes EditableText (Qt6, some GTK).
/// For Qt5, which doesn't expose widgets when unfocused, this will return Err.
/// Returns Ok if an editable was found and text was set, Err otherwise.
pub fn type_into_editable(pid: u32, text: &str) -> Result<()> {
    native::type_into_editable(pid, text)
}

/// Type into the exact indexed editable from the caller's accessibility snapshot.
pub fn type_into_editable_at(pid: u32, idx: usize, text: &str) -> Result<()> {
    native::type_into_editable_at(pid, idx, text)
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

/// Classify what holds keyboard focus so `type_text` can target the focused
/// widget (the thing just clicked) instead of the first editable anywhere:
/// `Some(true)` = focused editable, `Some(false)` = focused non-editable input
/// (spreadsheet cell, terminal, canvas), `None` = nothing focused / unreachable.
pub fn focused_is_editable(pid: u32) -> Result<Option<bool>> {
    native::focused_is_editable(pid)
}

pub fn get_element_bounds(pid: u32, idx: usize) -> Result<(i32, i32, u32, u32)> {
    native::get_element_bounds(pid, idx)
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Minimal X11 property-based tree (fallback when AT-SPI is unavailable).
fn walk_via_x11_properties(xid: u64, query: Option<&str>) -> AtspiTreeResult {
    use x11rb::rust_connection::RustConnection;

    let (conn, _) = match RustConnection::connect(None) {
        Ok(r) => r,
        Err(_) => {
            return AtspiTreeResult {
                tree_markdown: String::new(),
                nodes: vec![],
                bounds: vec![],
            }
        }
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
        name: if title.is_empty() {
            None
        } else {
            Some(title.clone())
        },
        value: None,
        checked: None,
        description: if wm_class.is_empty() {
            None
        } else {
            Some(wm_class.clone())
        },
        actions: vec!["activate".into()],
        element_key: xid,
        depth: 0,
        parent_element_index: None,
        in_web_content: false,
    };
    md.push_str(&format!(
        "- [0] window \"{}\" [actions=[activate]]\n",
        title
    ));
    nodes.push(root_node);

    let raw_md = md;
    let tree_markdown = if let Some(q) = query {
        filter_tree(&raw_md, q)
    } else {
        raw_md
    };

    AtspiTreeResult {
        tree_markdown,
        nodes,
        bounds: vec![],
    }
}

fn get_x11_title(conn: &x11rb::rust_connection::RustConnection, window: u32) -> Option<String> {
    use x11rb::protocol::xproto::*;
    // Try _NET_WM_NAME first.
    let net_wm_name = conn
        .intern_atom(false, b"_NET_WM_NAME")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let utf8_string = conn
        .intern_atom(false, b"UTF8_STRING")
        .ok()?
        .reply()
        .ok()?
        .atom;
    if let Ok(reply) = conn
        .get_property(false, window, net_wm_name, utf8_string, 0, 1024)
        .ok()?
        .reply()
    {
        if !reply.value.is_empty() {
            return Some(String::from_utf8_lossy(&reply.value).into_owned());
        }
    }
    let reply = conn
        .get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)
        .ok()?
        .reply()
        .ok()?;
    Some(String::from_utf8_lossy(&reply.value).into_owned())
}

fn get_x11_wm_class(conn: &x11rb::rust_connection::RustConnection, window: u32) -> Option<String> {
    use x11rb::protocol::xproto::*;
    let reply = conn
        .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 512)
        .ok()?
        .reply()
        .ok()?;
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
        while ancestors.len() <= depth {
            ancestors.push("");
            last_emitted.push(None);
        }
        for d in (depth + 1)..ancestors.len() {
            last_emitted[d] = None;
        }
        ancestors[depth] = line;
        if line.to_lowercase().contains(&needle) {
            for d in 0..depth {
                if ancestors[d].is_empty() {
                    continue;
                }
                if last_emitted[d] == Some(ancestors[d]) {
                    continue;
                }
                last_emitted[d] = Some(ancestors[d]);
                output.push(ancestors[d]);
            }
            last_emitted[depth] = Some(line);
            output.push(line);
        }
    }
    if output.is_empty() {
        return String::new();
    }
    let mut r = output.join("\n");
    r.push('\n');
    r
}
