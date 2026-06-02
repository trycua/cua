//! Native AT-SPI access over D-Bus via the `atspi` crate (zbus).
//!
//! Replaces the previous `python3 -c "import pyatspi; ..."` subprocess bridge:
//! no Python, `pyatspi`, or GObject-introspection typelibs are needed at
//! runtime. The zbus calls are async, so each public entry point drives a
//! small shared Tokio runtime via `block_on` (callers already invoke these
//! from `tokio::task::spawn_blocking`, so blocking here is safe).
//!
//! Element indices match the markdown produced by [`walk_tree`]: a depth-first,
//! pre-order traversal of the target application's windows, numbering only the
//! nodes that advertise AT-SPI actions. `perform_action`, `set_value`, and
//! `get_element_bounds` index into that same ordered set.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Result};
use atspi::connection::AccessibilityConnection;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::proxy_ext::ProxyExt;
use atspi::{CoordType, Interface, State};

use super::AtspiNode;

/// Per-call D-Bus timeout: a single unresponsive accessible (common in large,
/// lazily-built trees like Chromium's) must not stall the whole walk.
const CALL_TIMEOUT: Duration = Duration::from_secs(3);
/// Overall budget for one tree walk / operation.
const OP_TIMEOUT: Duration = Duration::from_secs(25);

/// Run `fut` with [`CALL_TIMEOUT`]; `None` on timeout so the caller can skip
/// the node and keep walking rather than blocking forever.
async fn call<T>(fut: impl std::future::Future<Output = T>) -> Option<T> {
    tokio::time::timeout(CALL_TIMEOUT, fut).await.ok()
}

/// Emit a one-line diagnostic to stderr when `CUA_ATSPI_DEBUG` is set. The
/// driver's stderr is surfaced in the test logs, so this is how we see what the
/// native walk actually found in CI.
fn dbg_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CUA_ATSPI_DEBUG").is_some())
}
macro_rules! dlog {
    ($($arg:tt)*) => {
        if dbg_enabled() { eprintln!("[cua-atspi] {}", format!($($arg)*)); }
    };
}

/// Shared multi-threaded Tokio runtime for the blocking AT-SPI entry points.
// ── Workspace isolation for invisible focus steal ────────────────────────────

use std::sync::Mutex;

/// State for workspace-isolated activation: saved so restore can undo everything.
struct WorkspaceState {
    saved_active_window: Option<u64>,
    saved_current_desktop: Option<u32>,
    target_xid: u64,
    target_original_desktop: Option<u32>,
    isolation_desktop: Option<u32>,
}

static WORKSPACE_STATE: Mutex<Option<WorkspaceState>> = Mutex::new(None);

/// Try to activate `xid` invisibly by moving it to a background workspace first.
/// Returns `true` if workspace isolation succeeded, `false` if it fell back to
/// direct activation (single workspace, WM doesn't support EWMH desktops, etc.).
async fn try_workspace_isolated_activation(xid: u64) -> bool {
    // Query workspace info: how many desktops, which one is current, which one
    // the target window is on. If any query fails, workspace isolation isn't
    // available — fall through to direct activation.
    let num_desktops = match crate::x11::get_number_of_desktops() {
        Ok(Some(n)) if n > 1 => n,
        _ => {
            dlog!("workspace isolation: single/no desktops, using direct activation");
            // Fallback: direct activation without workspace isolation.
            let saved_active = crate::x11::get_active_window().ok().flatten();
            if crate::x11::activate_window(xid).is_ok() {
                tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
                // Save minimal state for restore (no workspace moves needed).
                *WORKSPACE_STATE.lock().unwrap() = Some(WorkspaceState {
                    saved_active_window: saved_active,
                    saved_current_desktop: None,
                    target_xid: xid,
                    target_original_desktop: None,
                    isolation_desktop: None,
                });
                return false;
            }
            return false;
        }
    };

    let current_desktop = match crate::x11::get_current_desktop() {
        Ok(Some(d)) => d,
        _ => {
            dlog!("workspace isolation: can't read current desktop, using direct activation");
            let saved_active = crate::x11::get_active_window().ok().flatten();
            if crate::x11::activate_window(xid).is_ok() {
                tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
                *WORKSPACE_STATE.lock().unwrap() = Some(WorkspaceState {
                    saved_active_window: saved_active,
                    saved_current_desktop: None,
                    target_xid: xid,
                    target_original_desktop: None,
                    isolation_desktop: None,
                });
                return false;
            }
            return false;
        }
    };

    let target_desktop = crate::x11::get_window_desktop(xid).ok().flatten();

    // Pick an isolation desktop: prefer the last workspace (least likely to be
    // visible), but any desktop != current works. If the target is already on a
    // non-current desktop, we can activate it there without moving it.
    let isolation_desktop = if let Some(td) = target_desktop {
        if td != current_desktop {
            // Target is already on a background workspace; activate it there.
            td
        } else {
            // Target is on the current workspace; move it to the last one.
            num_desktops - 1
        }
    } else {
        // Target desktop unknown; try the last workspace.
        num_desktops - 1
    };

    // Save current state so restore can undo everything.
    let saved_active = crate::x11::get_active_window().ok().flatten();
    let state = WorkspaceState {
        saved_active_window: saved_active,
        saved_current_desktop: Some(current_desktop),
        target_xid: xid,
        target_original_desktop: target_desktop,
        isolation_desktop: Some(isolation_desktop),
    };

    dlog!(
        "workspace isolation: current={} target_ws={:?} isolation={} num={}",
        current_desktop,
        target_desktop,
        isolation_desktop,
        num_desktops
    );

    // Move the target window to the isolation desktop if it's not already there.
    if target_desktop != Some(isolation_desktop) {
        if let Err(e) = crate::x11::set_window_desktop(xid, isolation_desktop) {
            dlog!("workspace isolation: set_window_desktop failed: {}, using direct activation", e);
            // Fallback: direct activation on the current desktop.
            if crate::x11::activate_window(xid).is_ok() {
                tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
                *WORKSPACE_STATE.lock().unwrap() = Some(WorkspaceState {
                    saved_active_window: saved_active,
                    saved_current_desktop: None,
                    target_xid: xid,
                    target_original_desktop: None,
                    isolation_desktop: None,
                });
                return false;
            }
            return false;
        }
        // Give the WM a moment to process the workspace move.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    // Activate the target window (now on the isolation desktop, invisible to user).
    if let Err(e) = crate::x11::activate_window(xid) {
        dlog!("workspace isolation: activate_window failed: {}", e);
        // Attempt to restore the window to its original workspace before bailing.
        if let Some(orig) = target_desktop {
            let _ = crate::x11::set_window_desktop(xid, orig);
        }
        return false;
    }

    // Give the toolkit time to register activation and expose its AT-SPI editable.
    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

    *WORKSPACE_STATE.lock().unwrap() = Some(state);
    true
}

/// Restore workspace and focus after workspace-isolated activation. Safe to call
/// even if try_workspace_isolated_activation returned false (it's a no-op then,
/// or restores just the active window if direct activation was used).
async fn restore_after_workspace_activation() {
    let state = WORKSPACE_STATE.lock().unwrap().take();
    let Some(state) = state else {
        return;
    };

    // If workspace isolation was used, move the target back to its original desktop.
    if let (Some(orig_desktop), Some(_iso_desktop)) = (state.target_original_desktop, state.isolation_desktop) {
        dlog!("restoring window {} to workspace {}", state.target_xid, orig_desktop);
        if let Err(e) = crate::x11::set_window_desktop(state.target_xid, orig_desktop) {
            dlog!("restore: set_window_desktop failed: {}", e);
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    // Restore the previously active window (whether workspace isolation or direct).
    if let Some(prev_xid) = state.saved_active_window {
        dlog!("restoring focus to window {}", prev_xid);
        if crate::x11::activate_window(prev_xid).is_ok() {
            // Give the WM time to process the focus change so the caller sees the
            // restored state immediately (test assertions check focus stayed put).
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("build AT-SPI tokio runtime")
    })
}

/// A node discovered during the pre-order walk, with its proxy retained so the
/// per-index operations can act on it without re-walking the tree.
struct Visited<'a> {
    depth: usize,
    role: String,
    /// Display text: the accessible `name`, or — for editable/text widgets that
    /// expose no name — the Text-interface content (where typed text lives).
    name: String,
    value: Option<String>,
    actions: Vec<String>,
    has_editable: bool,
    has_value: bool,
    has_component: bool,
    focused: bool,
    /// True when an ancestor is a web document (e.g. role "document web"),
    /// i.e. this node is page content rather than browser chrome.
    in_web_doc: bool,
    acc: AccessibleProxy<'a>,
}

/// Role names that denote embedded web/document content. An editable beneath
/// one of these is page content (the field a user means when typing into a
/// background browser) rather than browser chrome like the address bar.
fn is_document_role(role: &str) -> bool {
    let r = role.to_ascii_lowercase();
    r.contains("document") || r == "embedded"
}

/// Build an `AccessibleProxy` for an arbitrary (bus name, path) in the tree.
/// Uses owned `String`s for destination/path so the resulting `BusName`/
/// `ObjectPath` are `'static` and the proxy borrows only the connection.
async fn accessible_for<'a>(
    conn: &'a atspi::zbus::Connection,
    oref: &atspi::ObjectRefOwned,
) -> Result<AccessibleProxy<'a>> {
    let dest = oref
        .name_as_str()
        .ok_or_else(|| anyhow!("object has no bus name"))?
        .to_owned();
    let path = oref.path_as_str().to_owned();
    AccessibleProxy::builder(conn)
        .destination(dest)
        .map_err(|e| anyhow!("bad a11y destination: {e}"))?
        .path(path)
        .map_err(|e| anyhow!("bad a11y path: {e}"))?
        .build()
        .await
        .map_err(|e| anyhow!("AccessibleProxy build failed: {e}"))
}

/// Resolve the process id behind an application accessible's D-Bus name.
async fn pid_of(dbus: &atspi::zbus::fdo::DBusProxy<'_>, oref: &atspi::ObjectRefOwned) -> Option<u32> {
    let bus = atspi::zbus::names::BusName::try_from(oref.name_as_str()?.to_owned()).ok()?;
    dbus.get_connection_unix_process_id(bus).await.ok()
}

/// Locate the application accessible whose backing process is `pid`.
async fn app_for_pid<'a>(
    conn: &'a AccessibilityConnection,
    pid: u32,
) -> Result<Option<AccessibleProxy<'a>>> {
    let zconn = conn.connection();
    let root = conn
        .root_accessible_on_registry()
        .await
        .map_err(|e| anyhow!("registry root unavailable: {e}"))?;
    let dbus = atspi::zbus::fdo::DBusProxy::new(zconn)
        .await
        .map_err(|e| anyhow!("DBus proxy unavailable: {e}"))?;

    let apps = root.get_children().await.unwrap_or_default();
    dlog!("registry root has {} application(s); seeking pid {pid}", apps.len());
    for child in apps {
        let cpid = pid_of(&dbus, &child).await;
        dlog!("  app bus={:?} pid={:?}", child.name_as_str(), cpid);
        if cpid == Some(pid) {
            return Ok(Some(accessible_for(zconn, &child).await?));
        }
    }
    dlog!("no application accessible matched pid {pid}");
    Ok(None)
}

/// Depth-first, pre-order walk of an application's windows. Mirrors the old
/// pyatspi `walk`/`collect` traversal so element indices stay stable.
async fn collect_visited<'a>(
    conn: &'a AccessibilityConnection,
    pid: u32,
) -> Result<Option<Vec<Visited<'a>>>> {
    let app = match app_for_pid(conn, pid).await? {
        Some(a) => a,
        None => return Ok(None),
    };
    let zconn = conn.connection();

    // Stack of (object ref, depth, in_web_doc). Seed with the app's windows;
    // push children reversed so siblings pop left-to-right and each subtree
    // completes before the next sibling (pre-order). `in_web_doc` is inherited
    // from ancestors so editables in page content can be told from chrome.
    let mut stack: Vec<(atspi::ObjectRefOwned, usize, bool)> = match call(app.get_children()).await {
        Some(Ok(children)) => children.into_iter().rev().map(|r| (r, 0usize, false)).collect(),
        _ => Vec::new(),
    };

    let mut visited: Vec<Visited<'a>> = Vec::new();
    // Guard against pathological/looping trees.
    let mut budget = 5000usize;

    while let Some((oref, depth, in_web_doc)) = stack.pop() {
        if budget == 0 {
            dlog!("node budget exhausted; truncating walk");
            break;
        }
        budget -= 1;

        let acc = match accessible_for(zconn, &oref).await {
            Ok(a) => a,
            Err(_) => continue,
        };

        // Interfaces gate every other query; if even this times out the node is
        // unreachable, so skip it rather than stall.
        let ifaces = match call(acc.get_interfaces()).await {
            Some(Ok(i)) => i,
            _ => continue,
        };
        let has_action = ifaces.contains(Interface::Action);
        let has_editable = ifaces.contains(Interface::EditableText);
        let has_value = ifaces.contains(Interface::Value);
        let has_component = ifaces.contains(Interface::Component);
        let has_text = ifaces.contains(Interface::Text);

        // These four are independent — issue them concurrently to cut the
        // per-node round-trip cost (large trees like Chromium's have hundreds
        // of nodes, so sequential reads dominate the walk time).
        let (role_r, name_r, state_r, children_r) = tokio::join!(
            call(acc.get_role_name()),
            call(acc.name()),
            call(acc.get_state()),
            call(acc.get_children()),
        );
        let role = match role_r {
            Some(Ok(r)) => r,
            _ => String::new(),
        };
        let mut name = match name_r {
            Some(Ok(n)) => n,
            _ => String::new(),
        };
        let focused = matches!(state_r, Some(Ok(s)) if s.contains(State::Focused));

        // Collect action names, numeric value, and (crucially) Text-interface
        // content. Only touch `proxies` when an interface is actually present,
        // and drop the borrow before `acc` moves into `visited`.
        let mut actions: Vec<String> = Vec::new();
        let mut value: Option<String> = None;
        let mut text_content = String::new();
        if has_action || has_value || has_text {
            if let Some(Ok(proxies)) = call(acc.proxies()).await {
                if has_action {
                    if let Some(Ok(ap)) = call(proxies.action()).await {
                        let n = call(ap.n_actions()).await.and_then(|r| r.ok()).unwrap_or(0);
                        for i in 0..n {
                            if let Some(Ok(an)) = call(ap.get_name(i)).await {
                                actions.push(an);
                            }
                        }
                    }
                }
                if has_value {
                    if let Some(Ok(vp)) = call(proxies.value()).await {
                        value = call(vp.current_value()).await.and_then(|r| r.ok()).map(format_value);
                    }
                }
                // Text content is where editable/entry text (the typed string)
                // lives; `name` is usually empty for such widgets.
                if has_text {
                    if let Some(Ok(tp)) = call(proxies.text()).await {
                        let count = call(tp.character_count()).await.and_then(|r| r.ok()).unwrap_or(0);
                        if count > 0 {
                            let end = count.min(4096);
                            if let Some(Ok(t)) = call(tp.get_text(0, end)).await {
                                text_content = t;
                            }
                        }
                    }
                }
            }
        }

        // Surface Text content as the display name when the widget has no name.
        if name.trim().is_empty() && !text_content.trim().is_empty() {
            name = text_content;
        }

        // Children inherit web-document context, plus this node's own role.
        let child_in_web_doc = in_web_doc || is_document_role(&role);

        // Enqueue children (fetched above) before moving `acc` into `visited`.
        if let Some(Ok(children)) = children_r {
            for c in children.into_iter().rev() {
                stack.push((c, depth + 1, child_in_web_doc));
            }
        }

        visited.push(Visited {
            depth,
            role,
            name,
            value,
            actions,
            has_editable,
            has_value,
            has_component,
            focused,
            in_web_doc,
            acc,
        });
    }

    dlog!("walked pid {pid}: {} node(s)", visited.len());
    Ok(Some(visited))
}

/// Render visited nodes into the markdown + node list `walk_tree` returns.
/// Format matches the historical pyatspi output exactly so downstream parsing
/// (`extract_text_from_markdown`, `query_dom`) is unaffected.
fn render(visited: &[Visited<'_>]) -> (String, Vec<AtspiNode>) {
    let mut md = String::new();
    let mut nodes = Vec::new();
    let mut idx = 0usize;

    for v in visited {
        let indent = "  ".repeat(v.depth);
        if !v.actions.is_empty() {
            let act_str = v.actions.join(",");
            let val_part = match &v.value {
                Some(val) if !val.is_empty() => format!(" value=\"{val}\""),
                _ => String::new(),
            };
            md.push_str(&format!(
                "{indent}- [{idx}] {role} \"{name}\"{val_part} [actions=[{act_str}]]\n",
                role = v.role,
                name = v.name,
            ));
            nodes.push(AtspiNode {
                element_index: Some(idx),
                role: v.role.clone(),
                name: if v.name.is_empty() { None } else { Some(v.name.clone()) },
                value: v.value.clone().filter(|s| !s.is_empty()),
                description: None,
                actions: v.actions.clone(),
                element_key: idx as u64,
            });
            idx += 1;
        } else if !v.name.is_empty() {
            md.push_str(&format!(
                "{indent}- {role} = \"{name}\"\n",
                role = v.role,
                name = v.name,
            ));
        }
    }

    (md, nodes)
}

/// Format an AT-SPI numeric value like the historical `str(currentValue)`
/// (e.g. `1.0`), so `value="..."` fields stay byte-compatible.
fn format_value(v: f64) -> String {
    format!("{v:?}")
}

// ── Public (sync) entry points ───────────────────────────────────────────────

pub fn walk_tree(pid: u32) -> Result<Option<(String, Vec<AtspiNode>)>> {
    runtime().block_on(async {
        let work = async {
            let conn = AccessibilityConnection::new()
                .await
                .map_err(|e| anyhow!("AT-SPI connect failed: {e}"))?;
            match collect_visited(&conn, pid).await? {
                Some(visited) => Ok(Some(render(&visited))),
                None => Ok(None),
            }
        };
        match tokio::time::timeout(OP_TIMEOUT, work).await {
            Ok(r) => r,
            Err(_) => {
                dlog!("walk_tree timed out for pid {pid}");
                Ok(None)
            }
        }
    })
}

pub fn insert_text(pid: u32, text: &str) -> Result<bool> {
    runtime().block_on(async {
        let conn = AccessibilityConnection::new()
            .await
            .map_err(|e| anyhow!("AT-SPI connect failed: {e}"))?;
        let visited = match collect_visited(&conn, pid).await? {
            Some(v) => v,
            None => return Ok(false),
        };

        dlog!(
            "insert_text: {} node(s), {} editable, {} entry/text-role",
            visited.len(),
            visited.iter().filter(|v| v.has_editable).count(),
            visited.iter().filter(|v| v.role.contains("entry") || v.role.contains("text")).count(),
        );

        // Target priority:
        //   1. the focused editable (if the toolkit exposes focus),
        //   2. an editable inside web/document content — for a browser this is
        //      the page's field, not the address bar (which sorts first in the
        //      tree but is chrome),
        //   3. the first editable anywhere (covers single-field apps like a
        //      GTK dialog entry).
        let target = visited
            .iter()
            .find(|v| v.has_editable && v.focused)
            .or_else(|| visited.iter().find(|v| v.has_editable && v.in_web_doc))
            .or_else(|| visited.iter().find(|v| v.has_editable));

        // If no editable found (GTK4/most toolkits gate editables on focus), try
        // temporarily activating the window. Qt6's AT-SPI bridge exposes editables
        // unfocused, but GTK4 doesn't. This is the focus-free write workaround.
        //
        // IMPROVEMENT: Use workspace isolation to make the focus steal invisible.
        // Move the target window to a background workspace, activate it there (so
        // the user doesn't see the focus change), perform the write, then restore
        // everything. Falls back to direct activation if workspaces aren't available.
        if target.is_none() {
            dlog!("no editable found unfocused; attempting activation workaround");
            // Get the first window for this pid and activate it.
            let windows = crate::x11::list_windows(Some(pid));
            if let Some(win) = windows.first() {
                let xid = win.xid;
                
                // Try workspace isolation first (invisible focus steal).
                let workspace_isolated = try_workspace_isolated_activation(xid).await;
                
                if !workspace_isolated {
                    // Fallback: direct activation (visible but brief focus steal).
                    dlog!("workspace isolation unavailable; using direct activation");
                }
                
                // Common path: whether workspace-isolated or direct, the window is now
                // active (either on a background workspace or here). Re-walk the tree.
                if let Some(revisited) = collect_visited(&conn, pid).await? {
                    dlog!(
                        "post-activation: {} editable",
                        revisited.iter().filter(|v| v.has_editable).count()
                    );
                    let target_activated = revisited
                        .iter()
                        .find(|v| v.has_editable && v.focused)
                        .or_else(|| revisited.iter().find(|v| v.has_editable && v.in_web_doc))
                        .or_else(|| revisited.iter().find(|v| v.has_editable));

                    if let Some(t) = target_activated {
                        dlog!(
                            "insert target (post-activation): role={:?} focused={}",
                            t.role, t.focused
                        );
                        let proxies = t
                            .acc
                            .proxies()
                            .await
                            .map_err(|e| anyhow!("interface proxies unavailable: {e}"))?;
                        let et = proxies
                            .editable_text()
                            .await
                            .map_err(|e| anyhow!("EditableText unavailable: {e}"))?;

                        let off = match proxies.text().await {
                            Ok(tp) => tp.caret_offset().await.unwrap_or(0),
                            Err(_) => 0,
                        };
                        let len = text.chars().count() as i32;

                        let success = if et.insert_text(off, text, len).await.unwrap_or(false) {
                            true
                        } else {
                            et.set_text_contents(text).await.unwrap_or(false)
                        };

                        // Restore workspace and focus (no-op if direct activation was used).
                        restore_after_workspace_activation().await;

                        return Ok(success);
                    }
                }

                // Restore even if insertion failed.
                restore_after_workspace_activation().await;
            }

            // Activation workaround didn't help; return false.
            return Ok(false);
        }
        }

        let target = target.unwrap();
        dlog!(
            "insert target: role={:?} in_web_doc={} focused={}",
            target.role, target.in_web_doc, target.focused
        );

        let proxies = target
            .acc
            .proxies()
            .await
            .map_err(|e| anyhow!("interface proxies unavailable: {e}"))?;
        let et = proxies
            .editable_text()
            .await
            .map_err(|e| anyhow!("EditableText unavailable: {e}"))?;

        let off = match proxies.text().await {
            Ok(tp) => tp.caret_offset().await.unwrap_or(0),
            Err(_) => 0,
        };
        let len = text.chars().count() as i32;

        if et.insert_text(off, text, len).await.unwrap_or(false) {
            return Ok(true);
        }
        Ok(et.set_text_contents(text).await.unwrap_or(false))
    })
}

pub fn perform_action(pid: u32, idx: usize) -> Result<String> {
    runtime().block_on(async {
        let conn = AccessibilityConnection::new()
            .await
            .map_err(|e| anyhow!("AT-SPI connect failed: {e}"))?;
        let visited = collect_visited(&conn, pid)
            .await?
            .ok_or_else(|| anyhow!("no AT-SPI application for pid {pid}"))?;
        let action_nodes: Vec<&Visited> = visited.iter().filter(|v| !v.actions.is_empty()).collect();
        let target = action_nodes
            .get(idx)
            .ok_or_else(|| anyhow!("element {idx} not found (total: {})", action_nodes.len()))?;

        let ap = target
            .acc
            .proxies()
            .await
            .map_err(|e| anyhow!("interface proxies unavailable: {e}"))?
            .action()
            .await
            .map_err(|e| anyhow!("Action unavailable: {e}"))?;
        ap.do_action(0)
            .await
            .map_err(|e| anyhow!("doAction failed: {e}"))?;
        Ok(target.actions.first().cloned().unwrap_or_default())
    })
}

pub fn set_value(pid: u32, idx: usize, value: &str) -> Result<()> {
    runtime().block_on(async {
        let conn = AccessibilityConnection::new()
            .await
            .map_err(|e| anyhow!("AT-SPI connect failed: {e}"))?;
        let visited = collect_visited(&conn, pid)
            .await?
            .ok_or_else(|| anyhow!("no AT-SPI application for pid {pid}"))?;
        let action_nodes: Vec<&Visited> = visited.iter().filter(|v| !v.actions.is_empty()).collect();
        let target = action_nodes
            .get(idx)
            .ok_or_else(|| anyhow!("element {idx} not found (total: {})", action_nodes.len()))?;

        let proxies = target
            .acc
            .proxies()
            .await
            .map_err(|e| anyhow!("interface proxies unavailable: {e}"))?;

        if target.has_editable {
            if let Ok(et) = proxies.editable_text().await {
                if et.set_text_contents(value).await.unwrap_or(false) {
                    return Ok(());
                }
            }
        }
        if target.has_value {
            let v: f64 = value
                .parse()
                .map_err(|_| anyhow!("value '{value}' is not numeric for a Value element"))?;
            proxies
                .value()
                .await
                .map_err(|e| anyhow!("Value unavailable: {e}"))?
                .set_current_value(v)
                .await
                .map_err(|e| anyhow!("setCurrentValue failed: {e}"))?;
            return Ok(());
        }
        Err(anyhow!("element {idx} exposes neither EditableText nor Value"))
    })
}

pub fn get_element_bounds(pid: u32, idx: usize) -> Result<(i32, i32, u32, u32)> {
    runtime().block_on(async {
        let conn = AccessibilityConnection::new()
            .await
            .map_err(|e| anyhow!("AT-SPI connect failed: {e}"))?;
        let visited = collect_visited(&conn, pid)
            .await?
            .ok_or_else(|| anyhow!("no AT-SPI application for pid {pid}"))?;
        let action_nodes: Vec<&Visited> = visited.iter().filter(|v| !v.actions.is_empty()).collect();
        let target = action_nodes
            .get(idx)
            .ok_or_else(|| anyhow!("element {idx} not found"))?;
        if !target.has_component {
            return Err(anyhow!("element {idx} exposes no Component interface"));
        }
        let comp = target
            .acc
            .proxies()
            .await
            .map_err(|e| anyhow!("interface proxies unavailable: {e}"))?
            .component()
            .await
            .map_err(|e| anyhow!("Component unavailable: {e}"))?;
        let (x, y, w, h) = comp
            .get_extents(CoordType::Screen)
            .await
            .map_err(|e| anyhow!("getExtents failed: {e}"))?;
        Ok((x, y, w.max(0) as u32, h.max(0) as u32))
    })
}
