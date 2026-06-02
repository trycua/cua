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

use anyhow::{anyhow, Result};
use atspi::connection::AccessibilityConnection;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::proxy_ext::ProxyExt;
use atspi::{CoordType, Interface, State};

use super::AtspiNode;

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
    name: String,
    value: Option<String>,
    actions: Vec<String>,
    has_editable: bool,
    has_value: bool,
    has_component: bool,
    focused: bool,
    acc: AccessibleProxy<'a>,
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

    for child in root.get_children().await.unwrap_or_default() {
        if pid_of(&dbus, &child).await == Some(pid) {
            return Ok(Some(accessible_for(zconn, &child).await?));
        }
    }
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

    // Stack of (object ref, depth). Seed with the app's windows; push children
    // reversed so siblings pop left-to-right and each subtree completes before
    // the next sibling (pre-order).
    let mut stack: Vec<(atspi::ObjectRefOwned, usize)> = app
        .get_children()
        .await
        .unwrap_or_default()
        .into_iter()
        .rev()
        .map(|r| (r, 0usize))
        .collect();

    let mut visited: Vec<Visited<'a>> = Vec::new();

    while let Some((oref, depth)) = stack.pop() {
        let acc = match accessible_for(zconn, &oref).await {
            Ok(a) => a,
            Err(_) => continue,
        };

        let ifaces = acc.get_interfaces().await.unwrap_or_default();
        let has_action = ifaces.contains(Interface::Action);
        let has_editable = ifaces.contains(Interface::EditableText);
        let has_value = ifaces.contains(Interface::Value);
        let has_component = ifaces.contains(Interface::Component);
        let has_text = ifaces.contains(Interface::Text);

        let role = acc.get_role_name().await.unwrap_or_default();
        let name = acc.name().await.unwrap_or_default();

        // Collect action names and the numeric value (if any) up front, so the
        // borrowed `Proxies` is dropped before `acc` moves into `visited`.
        let mut actions: Vec<String> = Vec::new();
        let mut value: Option<String> = None;
        if has_action || has_value {
            if let Ok(proxies) = acc.proxies().await {
                if has_action {
                    if let Ok(ap) = proxies.action().await {
                        let n = ap.n_actions().await.unwrap_or(0);
                        for i in 0..n {
                            if let Ok(an) = ap.get_name(i).await {
                                actions.push(an);
                            }
                        }
                    }
                }
                if has_value {
                    if let Ok(vp) = proxies.value().await {
                        value = vp.current_value().await.ok().map(format_value);
                    }
                }
            }
        }

        let focused = matches!(acc.get_state().await, Ok(s) if s.contains(State::Focused));

        // Enqueue children before moving `acc` into `visited`.
        let children = acc.get_children().await.unwrap_or_default();
        for c in children.into_iter().rev() {
            stack.push((c, depth + 1));
        }

        let _ = has_text;
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
            acc,
        });
    }

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
        let conn = AccessibilityConnection::new()
            .await
            .map_err(|e| anyhow!("AT-SPI connect failed: {e}"))?;
        match collect_visited(&conn, pid).await? {
            Some(visited) => Ok(Some(render(&visited))),
            None => Ok(None),
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

        // Prefer the focused editable; else the first editable in the tree.
        let target = visited
            .iter()
            .find(|v| v.has_editable && v.focused)
            .or_else(|| visited.iter().find(|v| v.has_editable));
        let target = match target {
            Some(t) => t,
            None => return Ok(false),
        };

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
