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
use atspi::proxy::component::ComponentProxy;
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
///
/// `cache_properties(No)` is load-bearing, not just an optimization: with the
/// zbus default (`Lazily`) the first property read on the proxy — our
/// `acc.name()` during the walk — makes zbus issue
/// `org.freedesktop.DBus.Properties.GetAll` (one argument: the interface name)
/// to warm the cache. Qt5's AT-SPI bridge (`AtSpiAdaptor::handleMessage`)
/// assumes every Properties call is `Get`/`Set` and unconditionally reads
/// `message.arguments().at(1)`; for a one-argument `GetAll` that index is out
/// of range, so the following `QVariant::toString()` dereferences garbage and
/// the Qt5 app *segfaults* (observed crash: `libQt5Core` via
/// `AtSpiAdaptor::handleMessage`). Qt6's bridge handles `GetAll`, which is why
/// only Qt5 crashed. Forcing `No` makes zbus issue per-property `Get` calls
/// (two arguments) instead, which Qt5 handles correctly — so the Qt5 window can
/// be walked and written without killing the app. Other toolkits are
/// unaffected (they already tolerate `GetAll`), and the sub-interface proxies
/// from `proxies()` already use `CacheProperties::No`.
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
        .cache_properties(atspi::zbus::proxy::CacheProperties::No)
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

        // Surface Text content as the display name when the widget has no
        // name. When an EDITABLE widget has a name (GTK entries name
        // themselves after their label/prompt), surface the Text content
        // as the value instead — otherwise the typed text is invisible in
        // snapshots and a background set_value can't be verified from the
        // tree. Restricted to editables so read-only text nodes (labels,
        // documents) keep their pre-existing snapshot shape.
        if name.trim().is_empty() && !text_content.trim().is_empty() {
            name = text_content;
        } else if value.is_none()
            && has_editable
            && !text_content.trim().is_empty()
            && text_content != name
        {
            value = Some(text_content);
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
            // Escape newlines/quotes: entry text is user-controlled and the
            // tree markdown is line-oriented — a literal newline or quote
            // would corrupt the node line for downstream parsers.
            let val_part = match &v.value {
                Some(val) if !val.is_empty() => {
                    let escaped = val.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "");
                    format!(" value=\"{escaped}\"")
                }
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

fn first_window_origin_for_pid(pid: u32) -> Option<(i32, i32)> {
    let window = crate::x11::list_windows(Some(pid)).into_iter().next()?;
    Some((window.x, window.y))
}

async fn component_extents_for_pid(
    comp: &ComponentProxy<'_>,
    pid: u32,
) -> Option<(i32, i32, u32, u32)> {
    let screen_extents = call(comp.get_extents(CoordType::Screen))
        .await
        .and_then(|r| r.ok());

    if let Some((x, y, w, h)) = screen_extents {
        if plausible_extents(x, y, w, h) && (x != 0 || y != 0) {
            return Some((x, y, w as u32, h as u32));
        }

        // GTK-on-XWayland can report every component at (0,0) in Screen
        // coordinates while still returning correct sizes. In that case try
        // the Window coordinate frame and translate by the X11 toplevel origin.
        if let Some((wx, wy, ww, wh)) = call(comp.get_extents(CoordType::Window))
            .await
            .and_then(|r| r.ok())
        {
            if plausible_extents(wx, wy, ww, wh) && (wx != 0 || wy != 0) {
                if let Some((origin_x, origin_y)) = first_window_origin_for_pid(pid) {
                    return Some((origin_x + wx, origin_y + wy, ww as u32, wh as u32));
                }
            }
        }

        if plausible_extents(x, y, w, h) {
            return Some((x, y, w as u32, h as u32));
        }
    }

    call(comp.get_extents(CoordType::Window))
        .await
        .and_then(|r| r.ok())
        .and_then(|(wx, wy, ww, wh)| {
            if !plausible_extents(wx, wy, ww, wh) {
                return None;
            }
            let (origin_x, origin_y) = first_window_origin_for_pid(pid)?;
            Some((origin_x + wx, origin_y + wy, ww as u32, wh as u32))
        })
}

fn plausible_extents(x: i32, y: i32, w: i32, h: i32) -> bool {
    x != i32::MIN && y != i32::MIN && x >= -16384 && y >= -16384 && w > 1 && h > 1
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

/// Pick the editable node to write into, by priority:
///   1. the focused editable (if the toolkit exposes focus),
///   2. an editable inside web/document content — for a browser this is the
///      page's field, not the address bar (which sorts first in the tree but is
///      chrome),
///   3. the first editable anywhere (covers single-field apps like a GTK dialog
///      entry, or a GTK4 GtkEntry).
fn pick_editable<'v, 'a>(visited: &'v [Visited<'a>]) -> Option<&'v Visited<'a>> {
    visited
        .iter()
        .find(|v| v.has_editable && v.focused)
        .or_else(|| visited.iter().find(|v| v.has_editable && v.in_web_doc))
        .or_else(|| visited.iter().find(|v| v.has_editable))
}

/// Try to write `text` into the best editable node in `visited` via AT-SPI
/// EditableText (GrabFocus first so the toolkit exposes the field on an
/// unfocused window's focused widget). Returns `Ok(true)` if the write landed,
/// `Ok(false)` if no editable was found / the EditableText write was rejected.
async fn write_into_editable(visited: &[Visited<'_>], text: &str) -> Result<bool> {
    let target = match pick_editable(visited) {
        Some(t) => t,
        None => return Ok(false),
    };
    dlog!(
        "insert target: role={:?} in_web_doc={} focused={} has_component={}",
        target.role, target.in_web_doc, target.focused, target.has_component
    );

    let proxies = target
        .acc
        .proxies()
        .await
        .map_err(|e| anyhow!("interface proxies unavailable: {e}"))?;

    // Try to grab focus on the widget via AT-SPI Component.GrabFocus.
    // This should give the widget internal keyboard focus without activating
    // the window, allowing GTK4 (and similar toolkits) to expose EditableText
    // on an unfocused window's focused widget.
    if target.has_component {
        if let Ok(comp) = proxies.component().await {
            match call(comp.grab_focus()).await {
                Some(Ok(true)) => dlog!("GrabFocus succeeded on {:?}", target.role),
                Some(Ok(false)) => dlog!("GrabFocus returned false on {:?}", target.role),
                Some(Err(e)) => dlog!("GrabFocus failed on {:?}: {}", target.role, e),
                None => dlog!("GrabFocus timed out on {:?}", target.role),
            }
        } else {
            dlog!("Component interface unavailable despite has_component=true");
        }
    } else {
        dlog!("Target has no Component interface, skipping GrabFocus");
    }

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
    if et.set_text_contents(text).await.unwrap_or(false) {
        return Ok(true);
    }
    Ok(false)
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

        // Primary attempt: write into an editable exposed by the current tree.
        // GrabFocus (inside write_into_editable) gives the widget internal
        // keyboard focus without activating the window, so toolkits that expose
        // EditableText on an unfocused window (Qt6, and GTK4 with GTK_A11Y=atspi)
        // accept the write here.
        if write_into_editable(&visited, text).await? {
            return Ok(true);
        }

        // GTK3 fallback: the toolkit exposes entry/text nodes in the tree (so
        // get_text reads work) but gates EditableText on focus/activation. Try
        // finding an entry/text role with Component bounds and use X11 click+type.
        dlog!("AT-SPI EditableText unavailable; checking for entry/text with Component for X11 fallback");

        let entry_candidate = visited
            .iter()
            .find(|v| {
                let r = v.role.to_ascii_lowercase();
                (r.contains("entry") || r.contains("text")) && v.has_component
            });

        if let Some(entry) = entry_candidate {
            dlog!(
                "GTK3 fallback: found entry role={:?} with Component; attempting X11 click+type",
                entry.role
            );

            // Get the entry widget's screen bounds via Component.GetExtents.
            if let Ok(proxies) = entry.acc.proxies().await {
                if let Ok(comp) = proxies.component().await {
                    if let Some(Ok((x, y, w, h))) = call(comp.get_extents(CoordType::Screen)).await {
                        // Click the center of the entry to establish widget focus (not window focus).
                        let cx = x + (w.max(0) / 2);
                        let cy = y + (h.max(0) / 2);
                        dlog!("GTK3 fallback: entry bounds ({x},{y} {w}x{h}), clicking center ({cx},{cy})");

                        // Get the window XID for this app so we can send X11 events to it.
                        let Some(xid) = entry_find_window_xid(pid).await else {
                            dlog!("GTK3 fallback: could not find window XID");
                            return Ok(false);
                        };

                        // Translate screen coords to window-local coords for XSendEvent.
                        let Some((wx, wy)) = screen_to_window_coords(xid, cx, cy) else {
                            dlog!("GTK3 fallback: screen-to-window coord translation failed");
                            return Ok(false);
                        };

                        dlog!("GTK3 fallback: window XID {xid}, local coords ({wx},{wy})");

                        // Click the entry to focus the widget (widget focus, not window focus).
                        if let Err(e) = crate::input::send_click(xid as u64, wx, wy, 1, 1) {
                            dlog!("GTK3 fallback: click failed: {e}");
                            return Ok(false);
                        };

                        // Small delay for the click to register and the widget to update focus.
                        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

                        // Now type via X11 XSendEvent — the entry widget has internal focus
                        // so it should accept the keystrokes even though the window is unfocused.
                        if let Err(e) = crate::input::send_type_text(xid as u64, text) {
                            dlog!("GTK3 fallback: send_type_text failed: {e}");
                            return Ok(false);
                        }

                        dlog!("GTK3 fallback: X11 click+type succeeded");
                        return Ok(true);
                    }
                }
            }
        }

        Ok(false)
    })
}

/// Find the window XID for a PID by listing its X11 windows.
async fn entry_find_window_xid(pid: u32) -> Option<u64> {
    use crate::x11::list_windows;

    // List X11 windows for that PID and return the first one.
    let windows = list_windows(Some(pid));
    let xid = windows.first()?.xid;
    Some(xid)
}

/// Translate screen coordinates to window-local coordinates.
fn screen_to_window_coords(xid: u64, screen_x: i32, screen_y: i32) -> Option<(i32, i32)> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;

    let (conn, _) = RustConnection::connect(None).ok()?;
    let window = xid as u32;

    // Get window geometry to find its screen position.
    let geom = conn.get_geometry(window).ok()?.reply().ok()?;

    // Translate to root coordinates (screen coords of window's origin).
    let trans = conn.translate_coordinates(window, geom.root, 0, 0).ok()?.reply().ok()?;

    // Window-local = screen - window_origin.
    Some((screen_x - trans.dst_x as i32, screen_y - trans.dst_y as i32))
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
        component_extents_for_pid(&comp, pid)
            .await
            .ok_or_else(|| anyhow!("getExtents returned no usable bounds for element {idx}"))
    })
}

/// Screen-coordinate bounds for every action node in the tree, keyed by the
/// same `element_index` used by [`walk_tree`]/`get_element_bounds`.
///
/// Walks the application once (unlike calling `get_element_bounds` per node,
/// which would reconnect and re-walk every time) and queries each node's
/// `Component.GetExtents(Screen)`. Nodes without a usable Component interface,
/// or whose extents query fails/times out, are silently skipped — the result is
/// best-effort and never errors on a per-node hiccup.
///
/// Returns `(element_index, x, y, width, height)` tuples.
pub fn get_all_element_bounds(pid: u32) -> Result<Vec<(usize, i32, i32, u32, u32)>> {
    runtime().block_on(async {
        let conn = AccessibilityConnection::new()
            .await
            .map_err(|e| anyhow!("AT-SPI connect failed: {e}"))?;
        let visited = collect_visited(&conn, pid)
            .await?
            .ok_or_else(|| anyhow!("no AT-SPI application for pid {pid}"))?;
        let action_nodes: Vec<&Visited> = visited.iter().filter(|v| !v.actions.is_empty()).collect();
        // Each element costs ~3 D-Bus round-trips (proxies + component +
        // GetExtents). Big trees (geany exposes ~787 nodes) would grind for
        // minutes and time out callers, so cap the walk; pre-order means the
        // first nodes are the window chrome / toolbars that are actually
        // visible, which is what bounds consumers (overlays, targeting) need.
        const MAX_BOUNDS_NODES: usize = 150;
        // Hard wall-clock budget for the whole collection: on pathological
        // trees individual D-Bus calls each burn up to CALL_TIMEOUT (geany's
        // unrealized nodes did exactly that), so a per-node cap alone can
        // still add up to minutes. Return whatever was collected in time.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut out = Vec::with_capacity(action_nodes.len().min(MAX_BOUNDS_NODES));
        for (idx, node) in action_nodes.iter().enumerate().take(MAX_BOUNDS_NODES) {
            if std::time::Instant::now() >= deadline {
                dlog!("get_all_element_bounds: 20s budget exhausted at node {idx}; returning {} bound(s)", out.len());
                break;
            }
            if !node.has_component {
                continue;
            }
            let proxies = match call(node.acc.proxies()).await {
                Some(Ok(p)) => p,
                _ => continue,
            };
            let comp = match call(proxies.component()).await {
                Some(Ok(c)) => c,
                _ => continue,
            };
            if let Some((x, y, w, h)) = component_extents_for_pid(&comp, pid).await {
                out.push((idx, x, y, w, h));
            }
        }
        Ok(out)
    })
}
