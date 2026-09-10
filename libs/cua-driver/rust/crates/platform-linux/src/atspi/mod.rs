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
    /// Enabled state when the accessibility backend returned a state set.
    pub enabled: Option<bool>,
    /// Toggle/selection state for selectable controls.
    pub selected: Option<bool>,
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
    /// D-Bus identity of the node (bus name, object path) for a native AT-SPI
    /// walk; `None` for the X11 property fallback and test fixtures. Lets a
    /// later per-index action re-open the exact object from a cached snapshot.
    pub object_ref: Option<native::ObjectRef>,
}

pub struct AtspiTreeResult {
    pub tree_markdown: String,
    pub nodes: Vec<AtspiNode>,
    pub bounds: Vec<(usize, i32, i32, u32, u32)>,
    /// True only for a native AT-SPI walk. The X11 property fallback is a
    /// partial discovery aid and must not prove verification predicates.
    pub trusted: bool,
    /// Machine-readable reason when a native walk failed in a way that is more
    /// specific than ordinary AT-SPI unavailability.
    pub degraded_reason: Option<String>,
    /// True when a caller-supplied `xid` was proven to correspond to exactly one
    /// of the application's top-levels, so these nodes are that window's and no
    /// other's. AT-SPI publishes one tree per process, so an application-scoped
    /// snapshot of a multi-window app carries every window's controls; callers
    /// that act on behalf of an exact native window must require this.
    pub window_scoped: bool,
    /// True when the walk stopped before exhausting the application's tree
    /// (deadline, node budget, unresponsive app, application lookup timeout).
    /// `nodes` is then a pre-order *prefix* of the real tree: every index in it
    /// is valid, but elements after the cut are simply absent.
    pub truncated: bool,
    /// Machine-readable reason when `truncated`: `timeout`, `node_budget`,
    /// `app_unresponsive`, `app_lookup_timeout`.
    pub truncation_reason: Option<String>,
    /// Nodes fully visited by the native walk (0 for the fallback tree).
    pub nodes_visited: usize,
    /// Nodes discovered but not visited when the walk stopped (lower bound).
    pub nodes_pending: usize,
    /// False when the bounds phase ran out of time; some indexed elements then
    /// carry no frame even though they exist.
    pub bounds_complete: bool,
    /// Wall time the native snapshot took (walk + bounds), in milliseconds.
    pub elapsed_ms: u128,
}

/// Total budget for callers that did not ask for one (browser flows, the
/// page tool, `walk_tree`). Bounds the WHOLE operation including retries —
/// previously the cold-registry retry loop could take 4 × 25 s.
pub const DEFAULT_WALK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25);

impl AtspiTreeResult {
    fn from_walked(walked: native::WalkedTree, query: Option<&str>) -> Self {
        let md = match query {
            Some(q) => filter_tree(&walked.markdown, q),
            None => walked.markdown,
        };
        let (truncated, truncation_reason, nodes_visited, nodes_pending) =
            match &walked.truncation {
                Some(t) => (true, Some(t.reason.to_owned()), t.visited, t.pending),
                None => (false, None, walked.nodes.len(), 0),
            };
        AtspiTreeResult {
            tree_markdown: md,
            nodes: walked.nodes,
            bounds: walked.bounds,
            trusted: true,
            degraded_reason: None,
            window_scoped: walked.window_scoped,
            truncated,
            truncation_reason,
            nodes_visited,
            nodes_pending,
            bounds_complete: walked.bounds_complete,
            elapsed_ms: walked.elapsed.as_millis(),
        }
    }
}

/// Walk the AT-SPI tree for a window identified by (pid, xid).
/// Falls back to a minimal X11 property tree if AT-SPI is unavailable.
pub fn walk_tree(pid: u32, xid: u64, query: Option<&str>) -> AtspiTreeResult {
    walk_tree_bounded(pid, xid, query, None, None)
}

/// Best-effort accessibility snapshot for synchronous trajectory evidence.
///
/// Recording brackets an action with before/after captures, so it must use a
/// smaller budget than the transport's tool-call deadline. Unlike the
/// interactive tree walker, evidence capture makes one attempt and accepts an
/// unavailable tree when the target renderer is blocked.
pub(crate) fn walk_tree_for_recording(
    pid: u32,
    xid: u64,
    timeout: std::time::Duration,
) -> AtspiTreeResult {
    if let Ok(Some(walked)) = native::walk_tree_bounded_with_timeout(pid, xid, None, None, timeout)
    {
        if !walked.markdown.is_empty() {
            return AtspiTreeResult::from_walked(walked, None);
        }
    }
    walk_via_x11_properties(xid, None)
}

/// Walk the AT-SPI tree with caller-supplied caps. `None` for either cap
/// means "use the walker's built-in default" (5 000 nodes; unlimited depth).
/// Issue #22865: caps protect against Electron / large web apps that
/// produce 10k+ element trees and blow context windows.
pub fn walk_tree_bounded(
    pid: u32,
    xid: u64,
    query: Option<&str>,
    max_elements: Option<usize>,
    max_depth: Option<usize>,
) -> AtspiTreeResult {
    walk_tree_bounded_within(pid, xid, query, max_elements, max_depth, DEFAULT_WALK_TIMEOUT)
}

/// [`walk_tree_bounded`] with an explicit wall-clock budget for the WHOLE
/// operation: the cold-registry retry loop, the tree walk and the bounds
/// phase all share `timeout`. A walk that runs out of budget returns the
/// partial tree with `truncated = true` rather than nothing.
pub fn walk_tree_bounded_within(
    pid: u32,
    xid: u64,
    query: Option<&str>,
    max_elements: Option<usize>,
    max_depth: Option<usize>,
    timeout: std::time::Duration,
) -> AtspiTreeResult {
    // Native AT-SPI (most complete). On a COLD launch the Qt6 (and some GTK)
    // AT-SPI bridge registers lazily — the first walk against a freshly
    // launched app can come back with just the root window (element_count=1,
    // no children) because `org.a11y.atspi.Registry` hasn't finished
    // enumerating the app's tree yet. Retry a few times with a short backoff
    // while the tree is suspiciously root-only, so the first get_window_state
    // after launch returns the real tree instead of an empty one. See #1927.
    // Every retry runs against what is LEFT of the caller's budget.
    const MAX_ATTEMPTS: usize = 4;
    const RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(150);
    let started = std::time::Instant::now();
    let mut native_failure = None;
    let mut last_partial: Option<native::WalkedTree> = None;
    for attempt in 0..MAX_ATTEMPTS {
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        match native::walk_tree_bounded_with_timeout(pid, xid, max_elements, max_depth, remaining)
        {
            Ok(Some(walked)) => {
                // `nodes.len() <= 1` == only the root window resolved: the
                // cold-registry symptom. Accept any real tree immediately; only
                // keep waiting on the degenerate case, and accept it anyway on the
                // final attempt rather than discarding a (minimal) valid result.
                // A truncated walk is never the cold-registry symptom — retrying
                // it would only burn the budget again.
                let is_last = attempt == MAX_ATTEMPTS - 1
                    || timeout.saturating_sub(started.elapsed()) <= RETRY_BACKOFF;
                if !walked.markdown.is_empty()
                    && (walked.nodes.len() > 1 || walked.truncation.is_some() || is_last)
                {
                    return AtspiTreeResult::from_walked(walked, query);
                }
                if walked.truncation.is_some() {
                    // Out of time before the application even answered.
                    last_partial = Some(walked);
                    break;
                }
                if !walked.markdown.is_empty() {
                    last_partial = Some(walked);
                }
            }
            Ok(None) => {}
            Err(error) => native_failure = Some(error.to_string()),
        }
        if attempt < MAX_ATTEMPTS - 1 {
            std::thread::sleep(RETRY_BACKOFF);
        }
    }
    if let Some(walked) = last_partial {
        if !walked.markdown.is_empty() || walked.truncation.is_some() {
            let mut result = AtspiTreeResult::from_walked(walked, query);
            if result.nodes.is_empty() {
                // Nothing came back in time: the X11 property tree is the
                // best discovery aid, but say WHY it is all we have.
                let mut fallback = walk_via_x11_properties(xid, query);
                fallback.truncated = true;
                fallback.truncation_reason = result.truncation_reason.take();
                fallback.elapsed_ms = started.elapsed().as_millis();
                fallback.degraded_reason = Some(
                    "atspi_walk_timed_out: the application did not answer AT-SPI within                      timeout_ms; retry with a larger timeout_ms"
                        .to_owned(),
                );
                return fallback;
            }
            return result;
        }
    }

    // Fallback: X11 window properties as minimal tree.
    let mut fallback = walk_via_x11_properties(xid, query);
    fallback.elapsed_ms = started.elapsed().as_millis();
    fallback.degraded_reason = native_failure.map(|error| format!("atspi_walk_failed: {error}"));
    fallback
}

/// Perform the first advertised action on element `idx` within pid's app tree.
/// Returns `Ok((action_name, suspected_noop))` on success — `suspected_noop`
/// is true when the actuated node looked like a silent no-op (a passive
/// display role, or no advertised action), so the caller can surface
/// `effect: "suspected_noop"`.
pub fn perform_action(pid: u32, idx: usize) -> Result<(String, bool)> {
    perform_action_in(pid, None, idx)
}

/// [`perform_action`] that first tries the element identity cached by the last
/// `get_window_state` snapshot of (pid, xid) — no re-walk — and only resolves
/// the index against a fresh walk when the cached object is gone.
pub fn perform_action_in(pid: u32, xid: Option<u64>, idx: usize) -> Result<(String, bool)> {
    if let Some(object_ref) = cache::cached_element(pid, xid, idx).and_then(|e| e.object_ref) {
        match native::perform_action_ref(&object_ref) {
            Ok(done) => return Ok(done),
            Err(error) => tracing::debug!(
                "cached element {idx} (pid {pid}) action failed, re-resolving: {error:#}"
            ),
        }
    }
    native::perform_action(pid, idx)
}

/// Give an indexed AT-SPI element keyboard focus without activating its window.
pub fn focus_element(pid: u32, idx: usize) -> Result<bool> {
    if let Some(object_ref) = cache::cached_element(pid, None, idx).and_then(|e| e.object_ref) {
        match native::focus_element_ref(&object_ref) {
            Ok(done) => return Ok(done),
            Err(error) => tracing::debug!(
                "cached element {idx} (pid {pid}) focus failed, re-resolving: {error:#}"
            ),
        }
    }
    native::focus_element(pid, idx)
}

pub use native::ScrollProgress;

pub fn scroll_element(
    pid: u32,
    idx: usize,
    direction: &str,
    amount: usize,
    by: cua_driver_contract::ScrollBy,
) -> Result<ScrollProgress> {
    native::scroll_element(pid, idx, direction, amount, by)
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
    perform_action_at_point_in(pid, 0, win_x, win_y)
}

/// [`perform_action_at_point`] for a known window, in three rungs that never
/// re-walk the tree first:
/// 1. the frames cached by the last `get_window_state` snapshot of (pid, xid);
/// 2. the toolkit's own `Component.GetAccessibleAtPoint` descent (O(depth));
/// 3. the historical bounded full-walk hit-test (last resort, short budget).
pub fn perform_action_at_point_in(
    pid: u32,
    xid: u64,
    win_x: i32,
    win_y: i32,
) -> Result<Option<String>> {
    if xid != 0 {
        if let Some((ox, oy)) = native::x11_window_origin(xid) {
            if let Some((idx, element)) = cache::hit_test(pid, xid, win_x + ox, win_y + oy) {
                if let Some(object_ref) = element.object_ref {
                    match native::perform_action_ref(&object_ref) {
                        Ok((action, _)) => return Ok(Some(action)),
                        Err(error) => tracing::debug!(
                            "cached hit-test element {idx} (pid {pid}) failed: {error:#}"
                        ),
                    }
                }
            }
        }
    }
    match native::perform_action_at_point_in(pid, xid, win_x, win_y) {
        Ok(Some(action)) => return Ok(Some(action)),
        Ok(None) => {}
        Err(error) => tracing::debug!("GetAccessibleAtPoint hit-test failed: {error:#}"),
    }
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
    if let Some(object_ref) = cache::cached_element(pid, None, idx).and_then(|e| e.object_ref) {
        match native::type_into_editable_ref(&object_ref, text) {
            Ok(()) => return Ok(()),
            Err(error) => tracing::debug!(
                "cached element {idx} (pid {pid}) editable write failed, re-resolving: {error:#}"
            ),
        }
    }
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
    if let Some(bounds) = cache::cached_element(pid, None, idx).and_then(|e| e.bounds) {
        return Ok(bounds);
    }
    native::get_element_bounds(pid, idx)
}

pub fn get_element_bounds_for_window(
    pid: u32,
    xid: u64,
    idx: usize,
) -> Result<(i32, i32, u32, u32)> {
    if let Some(bounds) = cache::cached_element(pid, Some(xid), idx).and_then(|e| e.bounds) {
        return Ok(bounds);
    }
    native::get_element_bounds_for_window(pid, xid, idx)
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
                trusted: false,
                degraded_reason: None,
                window_scoped: false,
                truncated: false,
                truncation_reason: None,
                nodes_visited: 0,
                nodes_pending: 0,
                bounds_complete: true,
                elapsed_ms: 0,
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
        enabled: None,
        selected: None,
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
        object_ref: None,
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
        trusted: false,
        // Built by reading this exact window's X11 properties, so it describes
        // one window by construction — but `trusted: false` still bars it from
        // proving anything a caller acts on.
        window_scoped: true,
        degraded_reason: None,
        truncated: false,
        truncation_reason: None,
        nodes_visited: 0,
        nodes_pending: 0,
        bounds_complete: true,
        elapsed_ms: 0,
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
