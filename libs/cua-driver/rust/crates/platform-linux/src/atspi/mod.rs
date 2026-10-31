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

use self::snapshot as cache;
use anyhow::Result;

pub mod native;
pub mod snapshot;
pub use native::ensure_listener_active;
pub use snapshot::Snapshots;

/// Stable address on one AT-SPI bus connection, including the owning frame.
/// Unique bus names prevent a restarted process from reusing an observed path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtspiIdentity {
    pub bus_name: String,
    pub path: String,
    pub frame_bus_name: String,
    pub frame_path: String,
}

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
    pub identity: Option<AtspiIdentity>,
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
    /// `app_unresponsive`, `app_lookup_timeout`, `huge_container`.
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
        let (truncated, truncation_reason, nodes_visited, nodes_pending) = match &walked.truncation
        {
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
    walk_tree_bounded_within(
        pid,
        xid,
        query,
        max_elements,
        max_depth,
        DEFAULT_WALK_TIMEOUT,
    )
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
        match native::walk_tree_bounded_with_timeout(pid, xid, max_elements, max_depth, remaining) {
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
                fallback.degraded_reason = Some(match fallback.truncation_reason.as_deref() {
                    Some("app_unresponsive") => "atspi_app_unresponsive: the application is \
                        registered with AT-SPI but did not answer GetChildren; its accessibility \
                        bridge may still be initialising (LibreOffice does this on first launch) \
                        - retry after a moment or with a larger timeout_ms"
                        .to_owned(),
                    _ => "atspi_walk_timed_out: the application did not answer AT-SPI within \
                        timeout_ms; retry with a larger timeout_ms"
                        .to_owned(),
                });
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
/// Returns `Ok((action_name, suspected_noop, unacknowledged))` on success —
/// `suspected_noop` is true when the actuated node looked like a silent
/// no-op (a passive display role, or no advertised action), so the caller
/// can surface `effect: "suspected_noop"`. `unacknowledged` is true when the
/// `doAction` D-Bus reply never arrived in time: the toolkit may still be
/// processing the request (e.g. behind a nested dialog loop), so neither
/// acceptance nor effect is known — the caller must report this as
/// `unverifiable` with an explicit note, never as a plain confirmed/dispatched
/// success.
pub fn perform_action(pid: u32, idx: usize) -> Result<(String, bool, bool)> {
    if let Some(observed) = cache::cached_element(pid, None, idx) {
        return TokenElement::new(idx, observed, WindowProof::NotRequired)
            .act(native::perform_action_ref, |idx| {
                native::perform_action(pid, idx)
            });
    }
    native::perform_action(pid, idx)
}

pub fn perform_action_walked(pid: u32, xid: u64, node: &AtspiNode) -> Result<(String, bool, bool)> {
    let (identity, object_ref) = walked_object(node)?;
    require_window_element(pid, xid, WindowElement::Identity(identity))?;
    native::perform_action_ref(object_ref)
}

/// [`perform_action_walked`] that runs `before_dispatch` immediately before the
/// action is sent, after the window-membership proof.
pub fn perform_action_walked_guarded(
    pid: u32,
    xid: u64,
    node: &AtspiNode,
    before_dispatch: impl Fn() -> Result<()>,
) -> Result<(String, bool, bool)> {
    let (identity, object_ref) = walked_object(node)?;
    require_window_element(pid, xid, WindowElement::Identity(identity))?;
    native::perform_action_ref_guarded(object_ref, before_dispatch)
}

/// The proven identity and the D-Bus object of a walked node: what an action
/// on that node addresses.
fn walked_object(node: &AtspiNode) -> Result<(AtspiIdentity, &native::ObjectRef)> {
    match (&node.identity, &node.object_ref) {
        (Some(identity), Some(object_ref)) => Ok((identity.clone(), object_ref)),
        _ => Err(anyhow::anyhow!(
            "the walked element has no proven AT-SPI identity"
        )),
    }
}

/// [`perform_action`] on the exact object a snapshot observed. Never re-walks
/// or retargets by index: when the object is gone the error downcasts to
/// [`native::CachedElementGone`] so the caller can refuse as stale.
pub fn perform_action_observed(element: &cache::CachedElement) -> Result<(String, bool, bool)> {
    let Some(object_ref) = element.object_ref.as_ref() else {
        return Err(
            native::CachedElementGone("observed element has no D-Bus address".into()).into(),
        );
    };
    native::perform_action_ref(object_ref)
}

/// Whether the exact object a snapshot observed is on screen right now
/// (its own state set carries `Showing`). Never re-walks.
pub fn element_showing_observed(element: &cache::CachedElement) -> Result<bool> {
    let Some(object_ref) = element.object_ref.as_ref() else {
        return Err(
            native::CachedElementGone("observed element has no D-Bus address".into()).into(),
        );
    };
    native::element_showing_ref(object_ref)
}

/// Give an indexed AT-SPI element keyboard focus without activating its window.
pub fn focus_element(pid: u32, idx: usize) -> Result<bool> {
    if let Some(observed) = cache::cached_element(pid, None, idx) {
        return focus_token(
            pid,
            &TokenElement::new(idx, observed, WindowProof::NotRequired),
        );
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
    native::scroll_element(pid, &WindowElement::Index(idx), direction, amount, by)
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
pub fn perform_action_at_point(pid: u32, win_x: i32, win_y: i32) -> Result<Option<AtPointHit>> {
    perform_action_at_point_in(pid, 0, win_x, win_y, false)
}

pub use native::is_focus_taking_role;
pub use native::AtPointHit;

/// [`perform_action_at_point`] for a known window, in three rungs that never
/// re-walk the tree first:
/// 1. the frames cached by the last `get_window_state` snapshot of (pid, xid);
/// 2. the toolkit's own `Component.GetAccessibleAtPoint` descent (O(depth));
/// 3. the historical bounded full-walk hit-test (last resort, short budget).
///
/// `skip_focus_roles`: the caller has a real pointer route to fall back to, so
/// an entry / spin button / table cell under the point (whose `doAction` does
/// not give it the keyboard focus a following `type_text` needs) is left
/// alone and `Ok(None)` is returned; see [`is_focus_taking_role`].
pub fn perform_action_at_point_in(
    pid: u32,
    xid: u64,
    win_x: i32,
    win_y: i32,
    skip_focus_roles: bool,
) -> Result<Option<AtPointHit>> {
    if xid != 0 {
        if let Some((ox, oy)) = native::x11_window_origin(xid) {
            if let Some(popup) = crate::input::popup_under_screen_point(win_x + ox, win_y + oy) {
                if popup.window != xid {
                    // A popup menu / popover covers the point. The window's
                    // own accessibles under it are not what the caller sees;
                    // the real pointer press reaches the popup item (or the
                    // caller names the popup as window_id and lands here
                    // with popup.window == xid).
                    tracing::debug!(
                        "point ({}, {}) is under {}; skipping the AT-SPI at-point tier for window {xid}",
                        win_x + ox,
                        win_y + oy,
                        popup.describe()
                    );
                    return Ok(None);
                }
            }
            if let Some((idx, element)) = cache::hit_test(pid, xid, win_x + ox, win_y + oy) {
                if skip_focus_roles && is_focus_taking_role(&element.role) {
                    return Ok(None);
                }
                if native::is_container_role(&element.role) {
                    // The cached snapshot's smallest covering element is a
                    // container (list / icon view / pane): its activation
                    // opens the current selection, not the item under the
                    // point. Let the live hit-test find the item.
                    tracing::debug!(
                        "cached hit-test element {idx} (pid {pid}) is a {} container; skipping",
                        element.role
                    );
                } else if let Some(object_ref) = element.object_ref {
                    match native::perform_action_ref(&object_ref) {
                        Ok((action, _, _)) => {
                            return Ok(Some(AtPointHit::fired(
                                action,
                                element.role.clone(),
                                String::new(),
                                object_ref.path.clone(),
                            )))
                        }
                        Err(error) => tracing::debug!(
                            "cached hit-test element {idx} (pid {pid}) failed: {error:#}"
                        ),
                    }
                }
            }
        }
    }
    match native::perform_action_at_point_in(pid, xid, win_x, win_y, skip_focus_roles) {
        Ok(Some(hit)) => return Ok(Some(hit)),
        Ok(None) => {}
        Err(error) => tracing::debug!("GetAccessibleAtPoint hit-test failed: {error:#}"),
    }
    native::perform_action_at_point(pid, win_x, win_y, skip_focus_roles)
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

/// Set the text value of element `idx` within pid's app tree via AT-SPI.
/// Tries `EditableText.set_text_contents(value)` first, then
/// `Value.set_current_value(float)`.
pub fn set_value(pid: u32, idx: usize, value: &str) -> Result<()> {
    native::set_value(pid, idx, value)
}

/// A combo box or list without an editable value: pick the option `value`
/// names through the element's `Selection`. `None` when it lists no
/// selectable options (the caller keeps its no-value-route error).
fn select_option(object_ref: &native::ObjectRef, value: &str) -> Option<Result<()>> {
    match native::select_option_ref(object_ref, value) {
        Ok(native::OptionPick::Picked { .. }) => Some(Ok(())),
        Ok(native::OptionPick::NoMatch { options }) => Some(Err(no_such_option(value, &options))),
        Ok(native::OptionPick::Unavailable) => None,
        Err(error) => Some(Err(error)),
    }
}

fn no_such_option(value: &str, options: &[String]) -> anyhow::Error {
    anyhow::anyhow!(
        "{}: no option {value:?}; options: {options:?}",
        native::NO_SUCH_OPTION
    )
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

/// `(role, name)` of the accessible holding the widget focus, from the
/// focus-event log only (no tree walk). See `native::focused_control`.
pub fn focused_control(pid: u32) -> Option<(String, String)> {
    native::focused_control(pid)
}

pub fn get_element_bounds(pid: u32, idx: usize) -> Result<(i32, i32, u32, u32)> {
    if let Some(bounds) = cached_bounds(pid, None, idx) {
        return Ok(bounds);
    }
    native::get_element_bounds(pid, idx)
}

pub fn get_element_bounds_for_window(
    pid: u32,
    xid: u64,
    idx: usize,
) -> Result<(i32, i32, u32, u32)> {
    if let Some(bounds) = cached_bounds(pid, Some(xid), idx) {
        return Ok(bounds);
    }
    native::get_element_bounds_for_window(pid, xid, idx)
}

/// Bounds for element `idx` from the last snapshot of (pid, xid): the frame
/// the snapshot recorded, or — when the snapshot ran out of bounds budget —
/// one `GetExtents` on the cached object identity. `None` only when the
/// snapshot never saw the element (or the object is gone), in which case the
/// caller re-walks.
fn cached_bounds(pid: u32, xid: Option<u64>, idx: usize) -> Option<(i32, i32, u32, u32)> {
    let element = cache::cached_element(pid, xid, idx)?;
    if let Some(bounds) = element.bounds {
        return Some(bounds);
    }
    let object_ref = element.object_ref.as_ref()?;
    match native::element_bounds_ref(object_ref, pid, xid.unwrap_or(0), element.in_web_content) {
        Ok(bounds) => Some(bounds),
        Err(error) => {
            tracing::debug!(
                "cached element {idx} (pid {pid}) bounds lookup failed, re-resolving: {error:#}"
            );
            None
        }
    }
}

pub use native::{is_window_membership_unproven, WindowElement, WindowProof};

/// Whether `get_window_state` may bind this snapshot's elements to window
/// `xid` as element tokens. See [`native::snapshot_binds_window_for`].
pub fn snapshot_binds_window(pid: u32, xid: u64, window_scoped: bool, trusted: bool) -> bool {
    native::snapshot_binds_window_for(pid, xid, window_scoped, trusted)
}

/// Prove from a fresh walk that `element` still belongs to window `xid`
/// before acting for that window. See [`native::require_window_element`].
pub fn require_window_element(pid: u32, xid: u64, element: WindowElement) -> Result<WindowProof> {
    native::require_window_element(pid, xid, element)
}

/// The element an element token names, carried from token resolution to the
/// action. Every action addresses `observed`, the object the token's own
/// snapshot saw, never the index in another snapshot of the process.
#[derive(Clone, Debug)]
pub struct TokenElement {
    /// Position in the token's snapshot, for messages and re-resolution.
    pub index: usize,
    pub observed: cache::CachedElement,
    /// Where no window proof applies (X11, Hyprland, compositors without a
    /// GNOME Shell), an action whose observed object fails re-resolves
    /// `index` in a fresh application walk, as before. After a window proof
    /// it never does, so a gone object is a stale token.
    pub may_reresolve: bool,
}

impl TokenElement {
    pub fn new(index: usize, observed: cache::CachedElement, proof: WindowProof) -> Self {
        Self {
            index,
            observed,
            may_reresolve: proof == WindowProof::NotRequired,
        }
    }

    /// The D-Bus object every token action addresses first.
    pub fn object(&self) -> Result<&native::ObjectRef> {
        self.observed.object_ref.as_ref().ok_or_else(|| {
            native::CachedElementGone("observed element has no D-Bus address".into()).into()
        })
    }

    /// Act on the observed object. When that fails, re-resolve the index with
    /// `by_index` only if `may_reresolve` allows it.
    fn act<T>(
        &self,
        on_object: impl FnOnce(&native::ObjectRef) -> Result<T>,
        by_index: impl FnOnce(usize) -> Result<T>,
    ) -> Result<T> {
        let error = match self.object().and_then(on_object) {
            Ok(done) => return Ok(done),
            Err(error) => error,
        };
        if !self.may_reresolve {
            return Err(error);
        }
        tracing::debug!(
            "observed element {} action failed, re-resolving: {error:#}",
            self.index
        );
        by_index(self.index)
    }
}

/// Give the token's element keyboard focus without activating its window.
pub fn focus_token(pid: u32, token: &TokenElement) -> Result<bool> {
    token.act(native::focus_element_ref, |idx| {
        native::focus_element(pid, idx)
    })
}

/// Type into the token's editable.
pub fn type_into_token(pid: u32, token: &TokenElement, text: &str) -> Result<()> {
    token.act(
        |object| native::type_into_editable_ref(object, text),
        |idx| native::type_into_editable_at(pid, idx, text),
    )
}

/// [`set_value`] on the token's element. `Err` messages starting with
/// [`native::NO_VALUE_ROUTE`] mean the element has no accessibility write
/// route at all.
pub fn set_value_token(pid: u32, token: &TokenElement, value: &str) -> Result<()> {
    token
        .act(
            |object| match native::set_value_ref(object, value) {
                // A missing value route is the element's own answer, not a
                // failure to reach it, so it is never re-resolved.
                Err(error) if native::is_no_value_route(&error) => {
                    Ok(select_option(object, value).unwrap_or(Err(error)))
                }
                done => done.map(Ok),
            },
            |idx| Ok(native::set_value(pid, idx, value)),
        )
        .and_then(|done| done)
}

/// The open popup of the token's combo box: activate the option `value`
/// names. `Ok(None)` when the popup lists no options yet.
pub fn activate_option_token(token: &TokenElement, value: &str) -> Result<Option<String>> {
    match native::activate_option_ref(token.object()?, value)? {
        native::OptionPick::Picked { name } => Ok(Some(name)),
        native::OptionPick::NoMatch { options } => Err(no_such_option(value, &options)),
        native::OptionPick::Unavailable => Ok(None),
    }
}

/// Whether the popup of the token's combo box is open.
pub fn options_showing_token(token: &TokenElement) -> bool {
    token.object().is_ok_and(native::options_showing_ref)
}

/// Read the token element's current value/text (for the read-back after a
/// write). `None` when its object is gone or exposes neither `Value` nor
/// `Text`.
pub fn read_value_token(token: &TokenElement) -> Option<String> {
    native::read_value_ref(token.object().ok()?).ok().flatten()
}

/// Screen bounds of the token's element: the frame its snapshot recorded, or
/// one `GetExtents` on the observed object.
pub fn token_bounds(
    pid: u32,
    xid: Option<u64>,
    token: &TokenElement,
) -> Result<(i32, i32, u32, u32)> {
    if let Some(bounds) = token.observed.bounds {
        return Ok(bounds);
    }
    token.act(
        |object| {
            native::element_bounds_ref(object, pid, xid.unwrap_or(0), token.observed.in_web_content)
        },
        |idx| match xid {
            Some(xid) => native::get_element_bounds_for_window(pid, xid, idx),
            None => native::get_element_bounds(pid, idx),
        },
    )
}

/// Scroll the token's element. A fresh walk finds it by its observed
/// identity; only where `may_reresolve` allows it does a gone identity fall
/// back to the index.
pub fn scroll_token(
    pid: u32,
    token: &TokenElement,
    direction: &str,
    amount: usize,
    by: cua_driver_contract::ScrollBy,
) -> Result<ScrollProgress> {
    let observed = WindowElement::Identity(token.observed.identity.clone());
    match native::scroll_element(pid, &observed, direction, amount, by) {
        Err(error) if token.may_reresolve && error.is::<native::CachedElementGone>() => {
            let index = WindowElement::Index(token.index);
            native::scroll_element(pid, &index, direction, amount, by)
        }
        result => result,
    }
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
        identity: None,
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
        last_emitted[depth + 1..ancestors.len()].fill(None);
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

#[cfg(test)]
mod budget_tests {
    use super::*;

    fn walked(truncation: Option<native::Truncation>) -> native::WalkedTree {
        let node =
            |idx: usize, role: &str, name: &str, depth: usize, actions: Vec<String>| AtspiNode {
                element_index: Some(idx),
                role: role.into(),
                name: Some(name.into()),
                value: None,
                checked: None,
                enabled: Some(true),
                selected: None,
                description: None,
                actions,
                element_key: idx as u64,
                depth,
                parent_element_index: (depth > 0).then_some(0),
                in_web_content: false,
                identity: None,
                object_ref: None,
            };
        native::WalkedTree {
            markdown: "- [0] frame \"Untitled\"\n  - [1] push button \"OK\"\n".into(),
            nodes: vec![
                node(0, "frame", "Untitled", 0, vec![]),
                node(1, "push button", "OK", 1, vec!["click".into()]),
            ],
            bounds: vec![(1, 10, 10, 50, 20)],
            window_scoped: true,
            truncation,
            bounds_complete: false,
            elapsed: std::time::Duration::from_millis(1234),
        }
    }

    #[test]
    fn complete_walk_reports_no_truncation_and_counts_nodes() {
        let result = AtspiTreeResult::from_walked(walked(None), None);
        assert!(result.trusted);
        assert!(!result.truncated);
        assert_eq!(result.truncation_reason, None);
        assert_eq!(result.nodes_visited, 2);
        assert_eq!(result.nodes_pending, 0);
        assert!(!result.bounds_complete);
        assert_eq!(result.elapsed_ms, 1234);
        assert_eq!(result.bounds, vec![(1, 10, 10, 50, 20)]);
    }

    #[test]
    fn truncated_walk_carries_reason_and_pending_count_through_query_projection() {
        let result = AtspiTreeResult::from_walked(
            walked(Some(native::Truncation {
                reason: "timeout",
                visited: 2,
                pending: 17,
            })),
            Some("ok"),
        );
        assert!(result.truncated);
        assert_eq!(result.truncation_reason.as_deref(), Some("timeout"));
        assert_eq!(result.nodes_visited, 2);
        assert_eq!(result.nodes_pending, 17);
        // The query projection keeps the matching row plus its ancestor.
        assert!(result.tree_markdown.contains("push button \"OK\""));
        assert!(result.tree_markdown.contains("frame \"Untitled\""));
        // Nodes are the unfiltered prefix: indices stay valid for actuation.
        assert_eq!(result.nodes.len(), 2);
    }

    #[test]
    fn x11_fallback_is_never_truncated_or_trusted() {
        let result = walk_via_x11_properties(0, None);
        assert!(!result.trusted);
        assert!(!result.truncated);
        assert_eq!(result.nodes_visited, 0);
        assert!(result.bounds_complete);
    }
}

#[cfg(test)]
mod token_dispatch_tests {
    use super::*;

    fn token(proof: WindowProof) -> TokenElement {
        let path = "/window_b/text";
        TokenElement::new(
            3,
            cache::CachedElement {
                key: 0,
                identity: AtspiIdentity {
                    bus_name: ":1.42".into(),
                    path: path.into(),
                    frame_bus_name: ":1.42".into(),
                    frame_path: "/frame".into(),
                },
                object_ref: Some(native::ObjectRef {
                    bus: ":1.42".into(),
                    path: path.into(),
                }),
                role: "text".into(),
                in_web_content: false,
                bounds: None,
            },
            proof,
        )
    }

    fn gone(_: &native::ObjectRef) -> Result<String> {
        Err(native::CachedElementGone("cached element is gone".into()).into())
    }

    #[test]
    fn a_proven_token_never_re_resolves_its_index() {
        let error = token(WindowProof::Proven)
            .act(gone, |_| {
                panic!("re-resolved the index after a window proof")
            })
            .unwrap_err();
        assert!(error.is::<native::CachedElementGone>());
    }

    #[test]
    fn a_walked_action_addresses_the_node_it_walked_not_another_window_at_its_index() {
        // Window A's snapshot is retained with an element at the same index
        // as the consent control a walk of window B found.
        let pid = 515_252;
        let node = |path: &str| AtspiNode {
            element_index: Some(3),
            role: "push button".into(),
            name: Some("Cancel".into()),
            value: None,
            checked: None,
            enabled: Some(true),
            selected: None,
            description: None,
            actions: vec!["click".into()],
            element_key: 0,
            identity: Some(AtspiIdentity {
                bus_name: ":1.42".into(),
                path: path.into(),
                frame_bus_name: ":1.42".into(),
                frame_path: "/frame".into(),
            }),
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
            object_ref: Some(native::ObjectRef {
                bus: ":1.42".into(),
                path: path.into(),
            }),
        };
        cache::update_snapshot(pid, 1, &[node("/window_a/button")], &[]);
        let walked = node("/window_b/button");
        let (identity, object) = walked_object(&walked).expect("walked node is addressable");
        assert_eq!(identity.path, "/window_b/button");
        assert_eq!(object.path, "/window_b/button");
        // The PID-only lookup the earlier dispatch used names window A's node.
        let pid_only = cache::cached_element(pid, None, 3).and_then(|element| element.object_ref);
        assert_eq!(
            pid_only.map(|object| object.path).as_deref(),
            Some("/window_a/button")
        );
        cache::forget_window(pid, 1);

        let mut unproven = node("/window_b/button");
        unproven.identity = None;
        assert!(walked_object(&unproven).is_err());
    }

    #[test]
    fn an_unproven_session_re_resolves_only_after_the_observed_object_fails() {
        let unproven = token(WindowProof::NotRequired);
        let reresolved = unproven.act(gone, |idx| Ok(format!("index {idx}")));
        assert_eq!(reresolved.unwrap(), "index 3");
        let observed = unproven.act(
            |object| Ok(object.path.clone()),
            |_| panic!("re-resolved after the observed object answered"),
        );
        assert_eq!(observed.unwrap(), "/window_b/text");
    }
}
