//! Native AT-SPI access over D-Bus via the `atspi` crate (zbus).
//!
//! Replaces the previous `python3 -c "import pyatspi; ..."` subprocess bridge:
//! no Python, `pyatspi`, or GObject-introspection typelibs are needed at
//! runtime. The zbus calls are async, so each public entry point drives a
//! small shared Tokio runtime via `block_on` (callers already invoke these
//! from `tokio::task::spawn_blocking`, so blocking here is safe).
//!
//! Element indices match the markdown produced by [`walk_tree`]: a depth-first,
//! pre-order traversal of the target application's windows, numbering the
//! nodes that advertise AT-SPI actions OR a Value interface (see is_indexable). `perform_action`, `set_value`, and
//! `get_element_bounds` index into that same ordered set.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Result};
use atspi::connection::{AccessibilityConnection, P2P};
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

/// Drive an AT-SPI op `work` on the runtime, bounded by [`OP_TIMEOUT`].
///
/// Individual interface calls are each bounded by [`call`], and `app_for_pid` /
/// `collect_visited` carry their own deadlines — but not every internal await is
/// wrapped (e.g. the EditableText writes in `write_into_editable`, the proxy
/// builds in `app_for_pid`), and an app that holds a modal grab can leave one of
/// those unwrapped round-trips pending indefinitely. `walk_tree` already guards
/// itself this way; this helper applies the same backstop to every other public
/// entry point so a modal/wedged app can never hang the caller (the daemon, an
/// MCP client) past OP_TIMEOUT (#1936). On timeout it yields `on_timeout` — the
/// graceful "couldn't complete" value, so e.g. `type_text` falls back to XTEST.
fn bounded<T>(
    work: impl std::future::Future<Output = Result<T>>,
    on_timeout: impl FnOnce() -> Result<T>,
) -> Result<T> {
    runtime().block_on(async move {
        match tokio::time::timeout(OP_TIMEOUT, work).await {
            Ok(r) => r,
            Err(_) => on_timeout(),
        }
    })
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

static SHARED_CONNECTION: tokio::sync::OnceCell<AccessibilityConnection> =
    tokio::sync::OnceCell::const_new();

/// Keep one AT-SPI connection and registry registration alive for the daemon
/// lifetime. WebKitGTK only publishes its WebProcess accessibility subtree
/// while the registry reports an interested listener.
async fn shared_connection() -> Result<&'static AccessibilityConnection> {
    SHARED_CONNECTION
        .get_or_try_init(|| async {
            let conn = AccessibilityConnection::new()
                .await
                .map_err(|error| anyhow!("AT-SPI connect failed: {error}"))?;
            if let Err(error) = conn.add_registry_event::<atspi::ObjectEvents>().await {
                dlog!("AT-SPI object-event registration failed: {error}");
            }
            Ok(conn)
        })
        .await
}

/// Establish the process-lifetime listener before accessibility-aware apps are
/// launched. Idempotent; later calls reuse the same connection.
pub fn ensure_listener_active() -> Result<()> {
    let connect = || runtime().block_on(async { shared_connection().await.map(|_| ()) });
    if tokio::runtime::Handle::try_current().is_ok() {
        // The daemon builds its registry from its Tokio entry-point. Calling
        // Runtime::block_on there panics even though this module owns a separate
        // runtime, so initialize the AT-SPI connection on a plain thread and
        // wait for it before accessibility-aware apps can launch.
        std::thread::spawn(connect)
            .join()
            .map_err(|_| anyhow!("AT-SPI listener initialization thread panicked"))?
    } else {
        connect()
    }
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
    conn: &'a AccessibilityConnection,
    oref: &RawObjectRef,
) -> Result<AccessibleProxy<'a>> {
    // Keep the atspi crate's peer-to-peer path when this connection actually
    // knows the peer. Late WebKit WebProcess children are not in the initial
    // peer snapshot; object_as_accessible's bus fallback omits their destination
    // and targets the Accessible interface name instead. Build an explicit bus
    // proxy below for those late peers and for well-known references.
    if oref.name.starts_with(':') {
        let name = atspi::zbus::names::UniqueName::try_from(oref.name.clone())
            .map_err(|e| anyhow!("bad a11y unique name: {e}"))?;
        let bus_name = atspi::zbus::names::BusName::Unique(name.as_ref());
        if conn.get_peer(&bus_name).is_some() {
            let path = atspi::zbus::zvariant::ObjectPath::try_from(oref.path.clone())
                .map_err(|e| anyhow!("bad a11y path: {e}"))?;
            let object = atspi::ObjectRef::new_owned(name, path);
            return conn
                .object_as_accessible(&object)
                .await
                .map_err(|e| anyhow!("AccessibleProxy build failed: {e}"));
        }
    }
    AccessibleProxy::builder(conn.connection())
        .cache_properties(atspi::zbus::proxy::CacheProperties::No)
        .destination(oref.name.clone())
        .map_err(|e| anyhow!("bad a11y destination: {e}"))?
        .path(oref.path.clone())
        .map_err(|e| anyhow!("bad a11y path: {e}"))?
        .build()
        .await
        .map_err(|e| anyhow!("AccessibleProxy build failed: {e}"))
}

/// AT-SPI's `(so)` object references are documented as unique bus names, but
/// WebKitGTK uses its well-known WebProcess name for the embedded web tree.
/// Keep the wire values as strings while walking so zbus does not reject that
/// validly addressable well-known name before we can call it.
#[derive(Clone, Debug)]
struct RawObjectRef {
    name: String,
    path: String,
}

impl RawObjectRef {
    fn from_atspi(oref: &atspi::ObjectRefOwned) -> Option<Self> {
        Some(Self {
            name: oref.name_as_str()?.to_owned(),
            path: oref.path_as_str().to_owned(),
        })
    }
}

/// Read Accessible.GetChildren without deserializing the bus-name field as a
/// `UniqueName`. WebKitGTK's embedded WebProcess exposes a well-known name
/// containing a UUID; D-Bus can address it, but the stricter AT-SPI wrapper
/// rejects it as an invalid unique name.
async fn raw_children(
    conn: &atspi::zbus::Connection,
    oref: &RawObjectRef,
) -> Result<Vec<RawObjectRef>> {
    let proxy = atspi::zbus::Proxy::new(
        conn,
        oref.name.as_str(),
        oref.path.as_str(),
        "org.a11y.atspi.Accessible",
    )
    .await
    .map_err(|e| anyhow!("Accessible proxy unavailable: {e}"))?;
    let refs: Vec<(String, atspi::zbus::zvariant::OwnedObjectPath)> = proxy
        .call("GetChildren", &())
        .await
        .map_err(|e| anyhow!("Accessible.GetChildren failed: {e}"))?;
    Ok(refs
        .into_iter()
        .map(|(name, path)| RawObjectRef {
            name,
            path: path.to_string(),
        })
        .collect())
}

/// Resolve the process id behind an application accessible's D-Bus name.
async fn pid_of(
    dbus: &atspi::zbus::fdo::DBusProxy<'_>,
    oref: &atspi::ObjectRefOwned,
) -> Option<u32> {
    let bus = atspi::zbus::names::BusName::try_from(oref.name_as_str()?.to_owned()).ok()?;
    dbus.get_connection_unix_process_id(bus).await.ok()
}

/// Locate the application accessible whose backing process is `pid`.
async fn app_for_pid<'a>(
    conn: &'a AccessibilityConnection,
    pid: u32,
) -> Result<Option<AccessibleProxy<'a>>> {
    let zconn = conn.connection();
    // Every AT-SPI round-trip below can block on an app whose main loop isn't
    // servicing D-Bus — most commonly one holding a modal grab (an "Add/Edit/
    // Preferences" dialog). Without a bound, `GetConnectionUnixProcessID` stalls
    // on the zbus default (~25s) per such app, which made get_window_state and
    // type_text hang on real apps (#1936). Bound each step with CALL_TIMEOUT and
    // skip/return instead of stalling — for type_text this returns fast so the
    // tool falls back to XTEST, which still types into the focused dialog field.
    let root = match call(conn.root_accessible_on_registry()).await {
        Some(Ok(r)) => r,
        Some(Err(e)) => return Err(anyhow!("registry root unavailable: {e}")),
        None => {
            dlog!("registry root lookup timed out");
            return Ok(None);
        }
    };
    let dbus = atspi::zbus::fdo::DBusProxy::new(zconn)
        .await
        .map_err(|e| anyhow!("DBus proxy unavailable: {e}"))?;

    let apps = match call(root.get_children()).await {
        Some(r) => r.unwrap_or_default(),
        None => {
            dlog!("registry get_children timed out");
            return Ok(None);
        }
    };
    dlog!(
        "registry root has {} application(s); seeking pid {pid}",
        apps.len()
    );
    for child in apps {
        // A modal-grabbed app can't answer the pid query; skip it after
        // CALL_TIMEOUT rather than blocking the whole walk on it.
        let cpid = match call(pid_of(&dbus, &child)).await {
            Some(p) => p,
            None => {
                dlog!(
                    "  pid_of timed out for bus={:?}, skipping",
                    child.name_as_str()
                );
                continue;
            }
        };
        dlog!("  app bus={:?} pid={:?}", child.name_as_str(), cpid);
        if cpid == Some(pid) {
            let child = match RawObjectRef::from_atspi(&child) {
                Some(child) => child,
                None => continue,
            };
            return match call(accessible_for(conn, &child)).await {
                Some(r) => r.map(Some),
                None => {
                    dlog!("  accessible_for timed out for pid {pid}");
                    Ok(None)
                }
            };
        }
    }
    dlog!("no application accessible matched pid {pid}");
    Ok(None)
}

/// Depth-first, pre-order walk of an application's windows. Mirrors the old
/// pyatspi `walk`/`collect` traversal so element indices stay stable.
#[allow(dead_code)]
async fn collect_visited<'a>(
    conn: &'a AccessibilityConnection,
    pid: u32,
) -> Result<Option<Vec<Visited<'a>>>> {
    collect_visited_bounded(conn, pid, None, None).await
}

/// `collect_visited` with caller-supplied caps.
/// - `max_elements = None` keeps the historical 5 000-node budget.
/// - `max_depth = None` keeps depth uncapped (the historical behaviour);
///   `Some(d)` skips enqueueing children whose depth would exceed `d`.
/// Issue #22865: caps protect against Electron / large web apps that produce
/// 10k+ element trees and blow context windows.
async fn collect_visited_bounded<'a>(
    conn: &'a AccessibilityConnection,
    pid: u32,
    max_elements: Option<usize>,
    max_depth: Option<usize>,
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
    let mut stack: Vec<(RawObjectRef, usize, bool)> = match call(app.get_children()).await {
        Some(Ok(children)) => children
            .into_iter()
            .filter_map(|child| RawObjectRef::from_atspi(&child))
            .rev()
            .map(|r| (r, 0usize, false))
            .collect(),
        _ => Vec::new(),
    };

    let mut visited: Vec<Visited<'a>> = Vec::new();
    // Guard against pathological/looping trees. Defaults to 5 000 (the
    // historical hard-coded budget); callers can override via max_elements.
    let mut budget = max_elements.unwrap_or(5000usize);
    // Time budget alongside the node budget: when an app is unresponsive to
    // AT-SPI (most commonly because it holds a modal grab and isn't servicing
    // D-Bus), every per-node `call()` burns the full CALL_TIMEOUT before being
    // skipped, so the walk would otherwise grind for minutes. Callers that lack
    // their own OP_TIMEOUT (snapshot bounds, insert_text) relied on this
    // never happening — bound it here so the walk returns partial within
    // OP_TIMEOUT for every caller, instead of hanging get_window_state/type_text
    // on modal dialogs (#1936).
    let deadline = std::time::Instant::now() + OP_TIMEOUT;
    // Fast bail for an app that has stopped answering AT-SPI entirely (modal
    // grab): if several consecutive nodes each burn the full CALL_TIMEOUT, the
    // app is unresponsive and the remaining ~OP_TIMEOUT of walking would all
    // time out too. Give up after a few so type_text falls back to XTEST in a
    // few seconds rather than ~25s.
    let mut consecutive_timeouts = 0u32;

    while let Some((oref, depth, in_web_doc)) = stack.pop() {
        if budget == 0 {
            dlog!("node budget exhausted; truncating walk");
            break;
        }
        if std::time::Instant::now() >= deadline {
            dlog!("collect_visited time budget exhausted; returning partial walk");
            break;
        }
        budget -= 1;

        // accessible_for builds a proxy whose first use round-trips to the
        // target app. On a modal-grabbed (AT-SPI-unresponsive) app this is the
        // await that actually hangs, so it MUST carry the per-call timeout —
        // otherwise the loop never returns to the deadline check at the top and
        // the walk stalls past OP_TIMEOUT for callers without an outer guard
        // (snapshot bounds, insert_text). That was the residual #1936 hang.
        let acc = match call(accessible_for(conn, &oref)).await {
            Some(Ok(a)) => a,
            Some(Err(error)) => {
                dlog!("  accessible_for failed: {error:#}");
                continue;
            }
            None => {
                consecutive_timeouts += 1;
                if consecutive_timeouts >= 3 {
                    dlog!(
                        "{} consecutive AT-SPI timeouts (accessible_for); app unresponsive, bailing walk",
                        consecutive_timeouts
                    );
                    break;
                }
                continue;
            }
        };

        // Interfaces gate every other query; if even this times out the node is
        // unreachable, so skip it rather than stall.
        let ifaces = match call(acc.get_interfaces()).await {
            Some(Ok(i)) => {
                consecutive_timeouts = 0;
                i
            }
            // A completed-but-errored call is node-specific; keep walking.
            Some(Err(error)) => {
                dlog!("  get_interfaces failed: {error:#}");
                continue;
            }
            // A timeout means the app didn't answer in CALL_TIMEOUT. A run of
            // these means the whole app is wedged — bail so callers fall back.
            None => {
                consecutive_timeouts += 1;
                if consecutive_timeouts >= 3 {
                    dlog!(
                        "{} consecutive AT-SPI timeouts; app unresponsive, bailing walk",
                        consecutive_timeouts
                    );
                    break;
                }
                continue;
            }
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
            call(raw_children(zconn, &oref)),
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
                        value = call(vp.current_value())
                            .await
                            .and_then(|r| r.ok())
                            .map(format_value);
                    }
                }
                // Text content is where editable/entry text (the typed string)
                // lives; `name` is usually empty for such widgets.
                if has_text {
                    if let Some(Ok(tp)) = call(proxies.text()).await {
                        let count = call(tp.character_count())
                            .await
                            .and_then(|r| r.ok())
                            .unwrap_or(0);
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
        // Honor max_depth (#22865): skip enqueueing descendants whose depth
        // would exceed the cap.
        let descend = max_depth.map(|d| depth + 1 <= d).unwrap_or(true);
        if descend {
            match children_r {
                Some(Ok(children)) => {
                    for c in children.into_iter().rev() {
                        stack.push((c, depth + 1, child_in_web_doc));
                    }
                }
                Some(Err(error)) => dlog!("  get_children failed: {error:#}"),
                None => dlog!("  get_children timed out"),
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
///
/// `parent_at_depth` tracks the most recently emitted actionable index at
/// each depth, so descendants can look up their parent_element_index without
/// a second pass.
fn render(visited: &[Visited<'_>]) -> (String, Vec<AtspiNode>) {
    let mut md = String::new();
    let mut nodes = Vec::new();
    let mut idx = 0usize;
    // Sparse stack: parent_at_depth[d] = Some(idx) for the actionable node
    // most recently emitted at depth d. When a new node appears at depth d,
    // its parent_element_index is the closest ancestor at depth < d that has
    // an entry. We invalidate deeper entries on each emit so stale siblings
    // don't leak across subtrees.
    let mut parent_at_depth: Vec<Option<usize>> = Vec::new();

    for v in visited {
        let indent = "  ".repeat(v.depth);
        // Resolve parent: walk parent_at_depth from v.depth-1 down to 0.
        let parent_element_index = if v.depth == 0 {
            None
        } else {
            (0..v.depth)
                .rev()
                .find_map(|d| parent_at_depth.get(d).copied().flatten())
        };

        if is_indexable(v) {
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
                name: if v.name.is_empty() {
                    None
                } else {
                    Some(v.name.clone())
                },
                value: v.value.clone().filter(|s| !s.is_empty()),
                description: None,
                actions: v.actions.clone(),
                element_key: idx as u64,
                depth: v.depth,
                parent_element_index,
            });
            // Record this actionable index at its depth, and invalidate any
            // deeper entries from a previous subtree.
            while parent_at_depth.len() <= v.depth {
                parent_at_depth.push(None);
            }
            parent_at_depth[v.depth] = Some(idx);
            for deeper in (v.depth + 1)..parent_at_depth.len() {
                parent_at_depth[deeper] = None;
            }
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

/// Whether a walked node is exposed as an indexed, usable element.
///
/// Historically this was "the node advertises AT-SPI Actions" (buttons, menu
/// items, links). That silently dropped every **Value**-only widget — GTK
/// `GtkScale` sliders, scroll bars, spin buttons, progress bars expose the
/// `Value` interface but NO `Action`, while some text fields expose
/// `EditableText` without either. Omitting those interfaces makes controls the
/// driver can operate impossible to address by `element_index`.
///
/// This predicate is the single source of truth for the element-index space and
/// MUST be applied identically in `render` and in every `action_nodes` filter
/// (`perform_action`, `set_value`, `get_element_bounds`, snapshot bounds);
/// any divergence would desync indices between the snapshot and the operations.
fn is_indexable(v: &Visited) -> bool {
    is_indexable_capabilities(!v.actions.is_empty(), v.has_editable, v.has_value)
}

fn is_indexable_capabilities(has_action: bool, has_editable: bool, has_value: bool) -> bool {
    has_action || has_editable || has_value
}

// ── Public (sync) entry points ───────────────────────────────────────────────

pub fn walk_tree(pid: u32) -> Result<Option<(String, Vec<AtspiNode>)>> {
    walk_tree_bounded(pid, 0, None, None)
        .map(|snapshot| snapshot.map(|(markdown, nodes, _)| (markdown, nodes)))
}

/// Walk the AT-SPI tree with caller-supplied node + depth caps.
/// `max_elements = None` keeps the historical 5 000-node default; `max_depth
/// = None` keeps the historical unbounded depth. Issue #22865.
pub fn walk_tree_bounded(
    pid: u32,
    xid: u64,
    max_elements: Option<usize>,
    max_depth: Option<usize>,
) -> Result<Option<(String, Vec<AtspiNode>, Vec<(usize, i32, i32, u32, u32)>)>> {
    runtime().block_on(async {
        let walk = async {
            let conn = shared_connection().await?;
            collect_visited_bounded(conn, pid, max_elements, max_depth).await
        };
        let visited = match tokio::time::timeout(OP_TIMEOUT, walk).await {
            Ok(result) => result?,
            Err(_) => {
                dlog!("walk_tree timed out for pid {pid}");
                return Ok(None);
            }
        };
        let Some(visited) = visited else {
            return Ok(None);
        };
        let (markdown, nodes) = render(&visited);
        let bounds = element_bounds_for_visited(&visited, pid, xid).await;
        Ok(Some((markdown, nodes, bounds)))
    })
}

/// Enumerate top-level windows from the AT-SPI registry — the window-listing
/// fallback for Wayland compositors that DON'T implement
/// `zwlr_foreign_toplevel_management` (GNOME Mutter, KDE KWin). Native Wayland
/// apps have no X11 XID and Mutter/KWin expose no foreign-toplevel list, so
/// `wayland::list_windows` comes back empty there and the whole element flow
/// (get_window_state -> click by element_index) is unreachable — even though the
/// AT-SPI tree itself is keyed by PID and works fine (see `walk_tree_bounded`,
/// whose walk ignores the xid). This bridges that gap: it returns one
/// [`WindowInfo`] per application top-level frame, with a SYNTHETIC but stable
/// `xid`. Downstream `get_window_state` / `click` walk the tree by PID and never
/// dereference the xid against X11, so the synthetic value only needs to be
/// non-zero and to round-trip back from the caller.
pub fn list_windows(filter_pid: Option<u32>) -> Vec<crate::x11::WindowInfo> {
    use crate::x11::WindowInfo;
    runtime().block_on(async {
        let work = async {
            let conn = shared_connection().await?;
            let zconn = conn.connection();
            let root = match call(conn.root_accessible_on_registry()).await {
                Some(Ok(r)) => r,
                _ => return Ok(Vec::new()),
            };
            let dbus = atspi::zbus::fdo::DBusProxy::new(zconn)
                .await
                .map_err(|e| anyhow!("DBus proxy unavailable: {e}"))?;
            let apps = match call(root.get_children()).await {
                Some(Ok(a)) => a,
                _ => return Ok(Vec::new()),
            };
            let mut out: Vec<WindowInfo> = Vec::new();
            for app_ref in apps {
                // Skip apps that can't answer the pid query (modal-grabbed) and
                // apps that don't match the filter.
                let cpid = match call(pid_of(&dbus, &app_ref)).await {
                    Some(Some(p)) => p,
                    _ => continue,
                };
                if let Some(want) = filter_pid {
                    if cpid != want {
                        continue;
                    }
                }
                let app_ref = match RawObjectRef::from_atspi(&app_ref) {
                    Some(app_ref) => app_ref,
                    None => continue,
                };
                let app = match call(accessible_for(conn, &app_ref)).await {
                    Some(Ok(a)) => a,
                    _ => continue,
                };
                let app_name = call(app.name())
                    .await
                    .and_then(|r| r.ok())
                    .unwrap_or_default();
                let frames = match call(app.get_children()).await {
                    Some(Ok(c)) => c,
                    _ => Vec::new(),
                };
                let mut emitted = 0usize;
                for (i, frame_ref) in frames.iter().enumerate() {
                    let frame_ref = match RawObjectRef::from_atspi(frame_ref) {
                        Some(frame_ref) => frame_ref,
                        None => continue,
                    };
                    let frame = match call(accessible_for(conn, &frame_ref)).await {
                        Some(Ok(f)) => f,
                        _ => continue,
                    };
                    let role = call(frame.get_role_name())
                        .await
                        .and_then(|r| r.ok())
                        .unwrap_or_default();
                    if !matches!(
                        role.as_str(),
                        "frame" | "window" | "dialog" | "alert" | "file chooser"
                    ) {
                        continue;
                    }
                    let title = call(frame.name())
                        .await
                        .and_then(|r| r.ok())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| app_name.clone());
                    let geometry = match call(frame.proxies()).await {
                        Some(Ok(proxies)) => match call(proxies.component()).await {
                            Some(Ok(component)) => {
                                call(component.get_extents(CoordType::Screen))
                                    .await
                                    .and_then(|result| result.ok())
                            }
                            _ => None,
                        },
                        _ => None,
                    };
                    let (x, y, width, height) = geometry
                        .filter(|(_, _, width, height)| *width > 0 && *height > 0)
                        .map(|(x, y, width, height)| {
                            (x, y, width.max(0) as u32, height.max(0) as u32)
                        })
                        .unwrap_or((0, 0, 0, 0));
                    // Stable, non-zero, unique per (pid, frame ordinal).
                    let xid = (((cpid as u64) << 16) | (i as u64)).max(1);
                    out.push(WindowInfo {
                        xid,
                        pid: Some(cpid),
                        app_name: app_name.clone(),
                        title,
                        is_on_screen: width > 0 && height > 0,
                        z_index: None,
                        x,
                        y,
                        width,
                        height,
                    });
                    emitted += 1;
                }
                // App with no enumerable top-level frame still gets one handle so
                // the by-pid AT-SPI element flow stays reachable.
                if emitted == 0 {
                    out.push(WindowInfo {
                        xid: (cpid as u64).max(1),
                        pid: Some(cpid),
                        app_name: app_name.clone(),
                        title: app_name,
                        is_on_screen: true,
                        z_index: None,
                        x: 0,
                        y: 0,
                        width: 0,
                        height: 0,
                    });
                }
            }
            Ok::<_, anyhow::Error>(out)
        };
        match tokio::time::timeout(OP_TIMEOUT, work).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                dlog!("atspi list_windows failed: {e}");
                Vec::new()
            }
            Err(_) => {
                dlog!("atspi list_windows timed out");
                Vec::new()
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
/// EditableText. Returns `Ok(true)` if the write landed,
/// `Ok(false)` if no editable was found / the EditableText write was rejected.
async fn write_into_editable(visited: &[Visited<'_>], text: &str) -> Result<bool> {
    let target = match pick_editable(visited) {
        Some(t) => t,
        None => return Ok(false),
    };
    write_into_editable_target(target, text).await
}

async fn write_into_editable_target(target: &Visited<'_>, text: &str) -> Result<bool> {
    dlog!(
        "insert target: role={:?} in_web_doc={} focused={} has_component={}",
        target.role,
        target.in_web_doc,
        target.focused,
        target.has_component
    );

    let proxies = target
        .acc
        .proxies()
        .await
        .map_err(|e| anyhow!("interface proxies unavailable: {e}"))?;

    // A focus-free EditableText write is the strongest background route. Try it
    // before GrabFocus: WebKitGTK can invalidate the original proxy when focus
    // changes, and writing through that stale object then returns false.
    if write_through_editable_proxies(&proxies, text).await? {
        return Ok(true);
    }

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

    write_through_editable_proxies(&proxies, text).await
}

async fn write_through_editable_proxies(
    proxies: &atspi::proxy::proxy_ext::Proxies<'_>,
    text: &str,
) -> Result<bool> {
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

/// Write into the best editable exposed by the current AT-SPI tree without
/// falling through to synthetic X11 input.
pub fn type_into_editable(pid: u32, text: &str) -> Result<()> {
    bounded(
        async {
            let conn = shared_connection().await?;
            let visited = collect_visited(conn, pid)
                .await?
                .ok_or_else(|| anyhow!("no AT-SPI application for pid {pid}"))?;
            if write_into_editable(&visited, text).await? {
                Ok(())
            } else {
                Err(anyhow!("no writable AT-SPI element found for pid {pid}"))
            }
        },
        || Err(anyhow!("AT-SPI editable lookup timed out for pid {pid}")),
    )
}

/// Write into the exact indexed editable exposed by the caller's snapshot.
pub fn type_into_editable_at(pid: u32, idx: usize, text: &str) -> Result<()> {
    bounded(
        async {
            let conn = shared_connection().await?;
            let visited = collect_visited(conn, pid)
                .await?
                .ok_or_else(|| anyhow!("no AT-SPI application for pid {pid}"))?;
            let target = visited
                .iter()
                .filter(|node| is_indexable(node))
                .nth(idx)
                .ok_or_else(|| anyhow!("element {idx} not found (total: {})", visited.len()))?;
            if write_into_editable_target(target, text).await? {
                Ok(())
            } else {
                // GrabFocus can rebuild WebKitGTK's accessibility object. Walk
                // the same index space again and retry only that exact element;
                // never fall through to a different focused/first editable.
                let refreshed = collect_visited(conn, pid)
                    .await?
                    .ok_or_else(|| anyhow!("no AT-SPI application for pid {pid}"))?;
                let refreshed_target = refreshed
                    .iter()
                    .filter(|node| is_indexable(node))
                    .nth(idx)
                    .ok_or_else(|| {
                        anyhow!("element {idx} disappeared after AT-SPI focus refresh")
                    })?;
                if write_into_editable_target(refreshed_target, text).await? {
                    Ok(())
                } else {
                    Err(anyhow!(
                        "element {idx} is not writable through AT-SPI EditableText"
                    ))
                }
            }
        },
        || {
            Err(anyhow!(
                "AT-SPI editable write timed out for element {idx} in pid {pid}"
            ))
        },
    )
}

pub fn insert_text(pid: u32, text: &str) -> Result<bool> {
    bounded(
        async {
            let conn = shared_connection().await?;
            let visited = match collect_visited(conn, pid).await? {
                Some(v) => v,
                None => return Ok(false),
            };

            dlog!(
                "insert_text: {} node(s), {} editable, {} entry/text-role",
                visited.len(),
                visited.iter().filter(|v| v.has_editable).count(),
                visited
                    .iter()
                    .filter(|v| v.role.contains("entry") || v.role.contains("text"))
                    .count(),
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

            let entry_candidate = visited.iter().find(|v| {
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
                        if let Some(Ok((x, y, w, h))) =
                            call(comp.get_extents(CoordType::Screen)).await
                        {
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
        },
        || {
            dlog!("insert_text timed out for pid {pid}; falling back to synthetic typing");
            Ok(false)
        },
    )
}

/// Classify what holds keyboard focus in `pid`'s tree, so `type_text` can target
/// the thing the user just clicked rather than the first editable anywhere:
///   `Some(true)`  — a focused **editable** widget (a text entry/box). AT-SPI
///                   EditableText insertion targets it correctly.
///   `Some(false)` — a focused **non-editable** widget that still accepts typed
///                   input (a spreadsheet cell/grid, a terminal, a canvas). An
///                   AT-SPI editable search would grab the wrong field here (e.g.
///                   gnumeric's name box), so the caller should synth-type into
///                   the focused widget instead.
///   `None`        — nothing is focused (or the app is unreachable): fall back to
///                   the focus-free "first editable" path for background typing.
pub fn focused_is_editable(pid: u32) -> Result<Option<bool>> {
    bounded(
        async {
            let conn = shared_connection().await?;
            let visited = match collect_visited(conn, pid).await? {
                Some(v) => v,
                None => return Ok(None),
            };
            Ok(visited.iter().find(|v| v.focused).map(|v| v.has_editable))
        },
        || Ok(None),
    )
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
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;

    let (conn, _) = RustConnection::connect(None).ok()?;
    let window = xid as u32;

    // Get window geometry to find its screen position.
    let geom = conn.get_geometry(window).ok()?.reply().ok()?;

    // Translate to root coordinates (screen coords of window's origin).
    let trans = conn
        .translate_coordinates(window, geom.root, 0, 0)
        .ok()?
        .reply()
        .ok()?;

    // Window-local = screen - window_origin.
    Some((screen_x - trans.dst_x as i32, screen_y - trans.dst_y as i32))
}

pub fn perform_action(pid: u32, idx: usize) -> Result<(String, bool)> {
    bounded(
        async {
            let conn = shared_connection().await?;
            let visited = collect_visited(conn, pid)
                .await?
                .ok_or_else(|| anyhow!("no AT-SPI application for pid {pid}"))?;
            let action_nodes: Vec<&Visited> = visited.iter().filter(|v| is_indexable(v)).collect();
            let target = action_nodes.get(idx).ok_or_else(|| {
                anyhow!("element {idx} not found (total: {})", action_nodes.len())
            })?;

            // Suspected no-op: actuating `do_action(0)` on a passive display role
            // (a `label`/`static`/`image` indexed only for its Value interface) or a
            // node that advertises no action at all is the AT-SPI analogue of macOS'
            // "element does not advertise this action" — the call returns success but
            // likely changes nothing. Reuses the same passive-role detector
            // `select_click_target` leans on for the coordinate paths. The caller
            // turns this into `effect: "suspected_noop"` + an escalation hint.
            let suspected_noop = target.actions.is_empty() || is_passive_role(&target.role);

            let ap = target
                .acc
                .proxies()
                .await
                .map_err(|e| anyhow!("interface proxies unavailable: {e}"))?
                .action()
                .await
                .map_err(|e| anyhow!("Action unavailable: {e}"))?;
            let action = target.actions.first().cloned().unwrap_or_default();
            ap.do_action(0)
                .await
                .map_err(|e| anyhow!("doAction failed: {e}"))?;
            // AT-SPI's doAction acknowledgement can precede the renderer's
            // queued DOM mutation. Give WebKit/Chromium one short event-loop
            // turn before returning success so a caller's immediate external
            // state read observes the action it was told was delivered.
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok((action, suspected_noop))
        },
        || {
            Err(anyhow!(
                "perform_action timed out for pid {pid} (app unresponsive to AT-SPI)"
            ))
        },
    )
}

/// Invoke an indexed scroll target's directional AT-SPI action.
///
/// Chromium exposes scrollable web regions as named actions such as
/// `scrollDown`/`scrollForward`; using that accessibility route avoids the
/// X11 `Button5` event path that Chromium silently drops in background mode.
pub fn scroll_element(pid: u32, idx: usize, direction: &str, amount: usize) -> Result<()> {
    bounded(
        async {
            let conn = shared_connection().await?;
            let visited = collect_visited(conn, pid)
                .await?
                .ok_or_else(|| anyhow!("no AT-SPI application for pid {pid}"))?;
            let target = visited
                .iter()
                .filter(|v| is_indexable(v))
                .nth(idx)
                .ok_or_else(|| anyhow!("element {idx} not found (total: {})", visited.len()))?;
            let proxies = target
                .acc
                .proxies()
                .await
                .map_err(|e| anyhow!("interface proxies unavailable: {e}"))?;
            let wanted = match direction {
                "up" => ["scrollup", "scrollbackward"],
                "left" => ["scrollleft", "scrollbackward"],
                "right" => ["scrollright", "scrollforward"],
                _ => ["scrolldown", "scrollforward"],
            };
            let mut selected = None;
            let mut action_proxy = None;
            if let Ok(action) = proxies.action().await {
                let count = call(action.n_actions())
                    .await
                    .and_then(|result| result.ok())
                    .unwrap_or(0);
                for action_index in 0..count {
                    if let Some(Ok(name)) = call(action.get_name(action_index)).await {
                        let normalized: String = name
                            .chars()
                            .filter(|ch| ch.is_ascii_alphanumeric())
                            .flat_map(|ch| ch.to_lowercase())
                            .collect();
                        if wanted.iter().any(|candidate| *candidate == normalized) {
                            selected = Some(action_index);
                            break;
                        }
                    }
                }
                action_proxy = Some(action);
            }

            if let (Some(action), Some(action_index)) = (action_proxy, selected) {
                for _ in 0..amount.max(1) {
                    match call(action.do_action(action_index)).await {
                        Some(Ok(true)) => {}
                        Some(Ok(false)) => return Err(anyhow!("scroll action returned false")),
                        Some(Err(e)) => return Err(anyhow!("scroll action failed: {e}")),
                        None => return Err(anyhow!("scroll action timed out")),
                    }
                }
                return Ok(());
            }

            if target.has_value {
                let value = proxies
                    .value()
                    .await
                    .map_err(|e| anyhow!("Value interface unavailable: {e}"))?;
                let current = call(value.current_value())
                    .await
                    .and_then(|result| result.ok())
                    .ok_or_else(|| anyhow!("scroll value lookup timed out"))?;
                let minimum = call(value.minimum_value())
                    .await
                    .and_then(|result| result.ok())
                    .unwrap_or(current);
                let maximum = call(value.maximum_value())
                    .await
                    .and_then(|result| result.ok())
                    .unwrap_or(current);
                let increment = call(value.minimum_increment())
                    .await
                    .and_then(|result| result.ok())
                    .filter(|increment| *increment > 0.0)
                    .unwrap_or(1.0);
                let sign = if matches!(direction, "up" | "left") {
                    -1.0
                } else {
                    1.0
                };
                let next =
                    (current + sign * increment * amount.max(1) as f64).clamp(minimum, maximum);
                call(value.set_current_value(next))
                    .await
                    .and_then(|result| result.ok())
                    .ok_or_else(|| anyhow!("scroll value update timed out"))?;
                return Ok(());
            }

            Err(anyhow!(
                "element {idx} exposes neither directional scroll actions nor Value"
            ))
        },
        || Err(anyhow!("scroll_element timed out for pid {pid}")),
    )
}

/// Give an indexed element keyboard focus through AT-SPI Component.GrabFocus
/// without activating or raising its toplevel window.
pub fn focus_element(pid: u32, idx: usize) -> Result<bool> {
    bounded(
        async {
            let conn = shared_connection().await?;
            let visited = collect_visited(conn, pid)
                .await?
                .ok_or_else(|| anyhow!("no AT-SPI application for pid {pid}"))?;
            let target = visited
                .iter()
                .filter(|v| is_indexable(v))
                .nth(idx)
                .ok_or_else(|| anyhow!("element {idx} not found (total: {})", visited.len()))?;
            let proxies = target
                .acc
                .proxies()
                .await
                .map_err(|e| anyhow!("interface proxies unavailable: {e}"))?;
            let component = proxies
                .component()
                .await
                .map_err(|e| anyhow!("Component interface unavailable: {e}"))?;
            match call(component.grab_focus()).await {
                Some(Ok(focused)) => Ok(focused),
                Some(Err(e)) => Err(anyhow!("Component.GrabFocus failed for element {idx}: {e}")),
                None => Err(anyhow!("Component.GrabFocus timed out for element {idx}")),
            }
        },
        || Err(anyhow!("focus_element timed out for pid {pid}")),
    )
}

/// Resolve a window-local pixel `(win_x, win_y)` to the deepest actionable
/// AT-SPI element covering it and perform its primary action.
///
/// This is the no-focus-steal way to land a *pixel* click on toolkits that drop
/// synthetic X11 pointer events. GTK3/4 take input via XInput2, so neither the
/// background `XSendEvent` path (synthetic, `send_event=True` — toolkits ignore
/// it) nor XTEST (its core events don't reach an XI2-only client; on a headless
/// Xvfb it also can't move a real device) actually clicks a GTK button. AT-SPI
/// `doAction` does, without activating or raising the window — the same path the
/// `element_index` click already uses, here driven by coordinates instead.
///
/// Hit-testing uses `Component.GetExtents(CoordType::Window)` so the caller's
/// window-local coordinates are compared directly against window-local widget
/// bounds — no screen-origin guessing. The smallest-area containing node wins so
/// a click lands on the button, not its enclosing panel. Returns `Ok(Some(action))`
/// when an element was actuated, `Ok(None)` when no actionable element covers the
/// point (the caller then falls back to the synthetic X11 path).
pub fn perform_action_at_point(pid: u32, win_x: i32, win_y: i32) -> Result<Option<String>> {
    bounded(
        async {
            let conn = shared_connection().await?;
            let visited = match collect_visited(conn, pid).await? {
                Some(v) => v,
                None => return Ok(None),
            };

            // Collect actionable nodes whose window-local bounds contain the point,
            // then let `select_click_target` pick the innermost *real actuator* —
            // preferring a button over its slightly-smaller inner label (GTK4 nests
            // one inside every button; an area-only pick lands on the inert label
            // and `do_action` silently no-ops). Pre-order keeps containers ahead of
            // children, but the area/role split is what actually disambiguates.
            let mut frames: Vec<(usize, i32, i32, u32, u32, bool)> = Vec::new();
            for (i, v) in visited.iter().enumerate() {
                if v.actions.is_empty() || !v.has_component {
                    continue;
                }
                let Some(Ok(proxies)) = call(v.acc.proxies()).await else {
                    continue;
                };
                let Some(Ok(comp)) = call(proxies.component()).await else {
                    continue;
                };
                let Some(Ok((x, y, w, h))) = call(comp.get_extents(CoordType::Window)).await else {
                    continue;
                };
                if w <= 0 || h <= 0 {
                    continue;
                }
                frames.push((i, x, y, w as u32, h as u32, is_passive_role(&v.role)));
            }

            let Some(idx) = select_click_target(&frames, win_x, win_y) else {
                return Ok(None);
            };
            let target = &visited[idx];
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
            Ok(Some(target.actions.first().cloned().unwrap_or_default()))
        },
        || Ok(None),
    )
}

/// Vision/pixel click that actually lands — the Wayland answer (and a robust
/// GTK4 path generally). Maps a *screen* pixel to the smallest `element_index`
/// whose reconstructed screen frame covers it, then fires that element's
/// primary action via [`perform_action`].
///
/// Why not [`perform_action_at_point`]: that one hit-tests raw
/// `CoordType::Window` extents over an ad-hoc node set and `do_action`s the node
/// it resolves directly. On GTK4 that can land on an inner, non-actuating node
/// (a label inside the button) → a silent no-op that still returns `Some`
/// ("false success"). And on native Wayland there is no virtual-pointer click to
/// fall back to (Mutter drops synthetic pointer events). This routine instead
/// uses the SAME screen-frame reconstruction that `get_window_state` exposes to
/// the agent, via the GNOME Shell helper on Wayland and `_GTK_FRAME_EXTENTS` on
/// X11, and actuates by `element_index`, the click path already verified
/// working. So "click at pixel (x,y)" becomes "click the element the agent sees
/// there", with no pointer injection and no reliance on `CoordType::Screen`
/// (which GTK4 reports as (0,0)).
///
/// `screen_x`/`screen_y` are full-display screen pixels (what the vision
/// screenshot and `get_window_state` frames are in). Returns `Ok(Some(action))`
/// on a hit, `Ok(None)` when no element covers the point so the caller can fall
/// back to its native injection path.
pub fn perform_action_at_screen_point(
    pid: u32,
    xid: u64,
    screen_x: i32,
    screen_y: i32,
) -> Result<Option<String>> {
    bounded(
        async {
            let conn = shared_connection().await?;
            let visited = match collect_visited(conn, pid).await? {
                Some(v) => v,
                None => return Ok(None),
            };

            // Reconstruct each indexable element's SCREEN frame the same way
            // get_window_state does: WINDOW-relative extents (GTK4 reports these
            // correctly; Screen is (0,0)) plus the window's screen origin (the
            // GNOME Shell helper on Wayland, _GTK_FRAME_EXTENTS on X11). When no
            // offset resolves, fall back to CoordType::Screen (correct on Qt/GTK3).
            let offset = window_to_screen_offset(pid, xid);
            let coord = if offset.is_some() {
                CoordType::Window
            } else {
                CoordType::Screen
            };
            let (ox, oy) = offset.unwrap_or((0, 0));

            // (element_index, x, y, w, h, is_passive_label) over the SAME indexable
            // list `perform_action`/`get_window_state` use, so the chosen index
            // maps straight back to a verified `element_index` actuation.
            let action_nodes: Vec<&Visited> = visited.iter().filter(|v| is_indexable(v)).collect();
            let mut frames: Vec<(usize, i32, i32, u32, u32, bool)> = Vec::new();
            for (idx, node) in action_nodes.iter().enumerate() {
                if !node.has_component {
                    continue;
                }
                let Some(Ok(proxies)) = call(node.acc.proxies()).await else {
                    continue;
                };
                let Some(Ok(comp)) = call(proxies.component()).await else {
                    continue;
                };
                let Some(Ok((x, y, w, h))) = call(comp.get_extents(coord)).await else {
                    continue;
                };
                if x == i32::MIN || y == i32::MIN || w <= 1 || h <= 1 {
                    continue;
                }
                frames.push((
                    idx,
                    x + ox,
                    y + oy,
                    w as u32,
                    h as u32,
                    is_passive_role(&node.role),
                ));
            }

            let Some(idx) = select_click_target(&frames, screen_x, screen_y) else {
                return Ok(None);
            };
            let target = action_nodes[idx];
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
            Ok(Some(target.actions.first().cloned().unwrap_or_default()))
        },
        || Ok(None),
    )
}

/// Roles that draw text/graphics but don't *do* anything when actuated. GTK4
/// nests a `label` inside every `button` with a near-identical (slightly
/// smaller) frame, so an area-only hit-test lands on the inert label —
/// `do_action` is a silent no-op (the "false success"). Treat these as
/// last-resort click targets.
fn is_passive_role(role: &str) -> bool {
    matches!(
        role,
        "label" | "static" | "static text" | "separator" | "filler" | "image" | "icon"
    )
}

/// Pick the `element_index` to actuate for a click at `(px, py)`. `frames` are
/// `(element_index, x, y, w, h, is_passive_label)` in the same coordinate space
/// as the point. Prefers the smallest covering *real actuator*; only falls back
/// to a passive label if nothing else covers the point. Smallest-area within a
/// class wins so the click lands on the button, not its enclosing panel.
/// Right/bottom edges are exclusive (`px < x + w`). `None` if nothing covers it.
fn select_click_target(
    frames: &[(usize, i32, i32, u32, u32, bool)],
    px: i32,
    py: i32,
) -> Option<usize> {
    let mut best_active: Option<(i64, usize)> = None;
    let mut best_passive: Option<(i64, usize)> = None;
    for &(idx, x, y, w, h, passive) in frames {
        let (w, h) = (w as i32, h as i32);
        if px >= x && px < x + w && py >= y && py < y + h {
            let area = (w as i64) * (h as i64);
            let slot = if passive {
                &mut best_passive
            } else {
                &mut best_active
            };
            if slot.map(|(a, _)| area < a).unwrap_or(true) {
                *slot = Some((area, idx));
            }
        }
    }
    best_active.or(best_passive).map(|(_, idx)| idx)
}

pub fn set_value(pid: u32, idx: usize, value: &str) -> Result<()> {
    bounded(
        async {
            let conn = shared_connection().await?;
            let visited = collect_visited(conn, pid)
                .await?
                .ok_or_else(|| anyhow!("no AT-SPI application for pid {pid}"))?;
            let action_nodes: Vec<&Visited> = visited.iter().filter(|v| is_indexable(v)).collect();
            let target = action_nodes.get(idx).ok_or_else(|| {
                anyhow!("element {idx} not found (total: {})", action_nodes.len())
            })?;

            let proxies = target
                .acc
                .proxies()
                .await
                .map_err(|e| anyhow!("interface proxies unavailable: {e}"))?;

            // SetValue is a focus-free accessibility operation. Do not call
            // Component.GrabFocus here: GTK may activate and raise the entire
            // toplevel in response, violating the background contract. Toolkits
            // that expose EditableText only while focused must return an honest
            // unsupported error rather than changing desktop focus implicitly.
            if let Ok(et) = proxies.editable_text().await {
                // Replace whole contents (parity with the Windows/macOS set_value,
                // which overwrite rather than insert at the caret).
                if et.set_text_contents(value).await.unwrap_or(false) {
                    return Ok(());
                }
                // Some toolkits reject SetTextContents but accept an insert at the
                // caret offset; clear-then-insert as a fallback.
                let off = match proxies.text().await {
                    Ok(tp) => tp.caret_offset().await.unwrap_or(0),
                    Err(_) => 0,
                };
                let len = value.chars().count() as i32;
                if et.insert_text(off, value, len).await.unwrap_or(false) {
                    return Ok(());
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
            Err(anyhow!(
                "element {idx} exposes neither EditableText nor Value"
            ))
        },
        || {
            Err(anyhow!(
                "set_value timed out for pid {pid} (app unresponsive to AT-SPI)"
            ))
        },
    )
}

pub fn get_element_bounds(pid: u32, idx: usize) -> Result<(i32, i32, u32, u32)> {
    bounded(
        async {
            let conn = shared_connection().await?;
            let visited = collect_visited(conn, pid)
                .await?
                .ok_or_else(|| anyhow!("no AT-SPI application for pid {pid}"))?;
            let action_nodes: Vec<&Visited> = visited.iter().filter(|v| is_indexable(v)).collect();
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
            // Prefer WINDOW coords + a deterministic screen offset — fixes GTK4,
            // whose CoordType::Screen collapses every element to (0,0). Fall back to
            // Screen on Wayland / when no X11 window resolves (offset is None).
            match window_to_screen_offset(pid, 0) {
                Some((ox, oy)) => {
                    let (x, y, w, h) = comp
                        .get_extents(CoordType::Window)
                        .await
                        .map_err(|e| anyhow!("getExtents failed: {e}"))?;
                    Ok((x + ox, y + oy, w.max(0) as u32, h.max(0) as u32))
                }
                None => {
                    let (x, y, w, h) = comp
                        .get_extents(CoordType::Screen)
                        .await
                        .map_err(|e| anyhow!("getExtents failed: {e}"))?;
                    Ok((x, y, w.max(0) as u32, h.max(0) as u32))
                }
            }
        },
        || {
            Err(anyhow!(
                "get_element_bounds timed out for pid {pid} (app unresponsive to AT-SPI)"
            ))
        },
    )
}

/// Real on-screen origin (root-relative top-left) of an X11 window, or `None`
/// if it can't be resolved. Mirrors `list_windows`' geometry path.
fn x11_window_origin(xid: u64) -> Option<(i32, i32)> {
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;

    let (conn, _) = RustConnection::connect(None).ok()?;
    let window = xid as u32;
    let geom = conn.get_geometry(window).ok()?.reply().ok()?;
    let trans = conn
        .translate_coordinates(window, geom.root, 0, 0)
        .ok()?
        .reply()
        .ok()?;
    Some((trans.dst_x as i32, trans.dst_y as i32))
}

/// Read the GTK4 client-side-decoration shadow inset from the X11
/// `_GTK_FRAME_EXTENTS` property (`CARDINAL[4]` = left, right, top, bottom).
///
/// A GTK4 window is an outer X11 window whose *visible content* starts `left`
/// px in and `top` px down — the rest is the invisible CSD shadow. AT-SPI
/// `CoordType::Window` coordinates are relative to that content origin, so
/// reconstructing true screen coords needs this inset added to the X11 window
/// origin. Returns `None` (treated as no inset, i.e. `(0,0)`) for non-GTK /
/// server-side-decorated windows that don't set the property — which is also
/// how we tell GTK4-CSD apart from everyone else.
fn gtk_frame_extents(xid: u64) -> Option<(i32, i32)> {
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;

    let (conn, _) = RustConnection::connect(None).ok()?;
    // only_if_exists=true → atom is 0 when no client ever set the property.
    let atom = conn
        .intern_atom(true, b"_GTK_FRAME_EXTENTS")
        .ok()?
        .reply()
        .ok()?
        .atom;
    if atom == 0 {
        return None;
    }
    let reply = conn
        .get_property(false, xid as u32, atom, AtomEnum::CARDINAL, 0, 4)
        .ok()?
        .reply()
        .ok()?;
    let vals: Vec<u32> = reply.value32()?.collect();
    parse_gtk_frame_extents(&vals)
}

/// Parse a `_GTK_FRAME_EXTENTS` `CARDINAL[4]` (`[left, right, top, bottom]`) into
/// the `(left, top)` shadow inset. `None` when fewer than 4 values (property
/// absent or malformed). Split out from [`gtk_frame_extents`] so the index
/// mapping (left = `[0]`, top = `[2]`, *not* `[1]`/`[3]`) is unit-tested without
/// an X server.
fn parse_gtk_frame_extents(vals: &[u32]) -> Option<(i32, i32)> {
    if vals.len() < 4 {
        return None;
    }
    Some((vals[0] as i32, vals[2] as i32))
}

/// Additive screen-coordinate offset that turns an element's
/// `CoordType::Window` extents into true screen coordinates:
/// `screen = x11_window_origin + _GTK_FRAME_EXTENTS.(left,top) + window_xy`.
///
/// AT-SPI `CoordType::Window` coords are relative to the toolkit's *content*
/// toplevel. The content's screen position is the X11 window's root-relative
/// origin plus the GTK4 CSD shadow inset (`_GTK_FRAME_EXTENTS`). This is
/// deterministic and replaces the old frame-(0,0)-detection heuristic; it fixes
/// GTK4, whose `CoordType::Screen` collapses *every* element to (0,0) so a
/// constant offset could never separate them (GNOME/gtk a11y rework, issues
/// #1564 / #1739) — `CoordType::Window` returns the distinct per-widget offsets
/// instead.
///
/// **Gated on `_GTK_FRAME_EXTENTS` presence**: only GTK toolkits set that
/// property (for CSD), and only GTK's Screen extents are unreliable. Non-GTK
/// toolkits (Qt, etc.) have no such property *and* report correct Screen
/// extents, so we return `None` for them — callers keep the unchanged Screen
/// path and the WINDOW reconstruction can never regress a toolkit that was
/// already correct. Also returns `None` on native Wayland (clients may not
/// query screen origins, by design) or when no X11 window resolves.
fn window_to_screen_offset(pid: u32, xid: u64) -> Option<(i32, i32)> {
    if crate::wayland::is_wayland() {
        // Native Wayland: clients can't query a window's screen origin, and
        // AT-SPI CoordType::Screen collapses to (0,0) on Mutter. The bundled
        // `org.cua.WinRects` GNOME Shell extension supplies the window's screen
        // origin (`meta_window.get_frame_rect()`); combined with the per-widget
        // CoordType::Window coords (which GTK4 reports correctly on Wayland too)
        // this reconstructs real screen coords — the GNOME analogue of the X11
        // `_GTK_FRAME_EXTENTS` path below. `None` (no extension) keeps the
        // legacy Screen path (still (0,0), but no worse than before).
        return crate::wayland::shell_helper::window_origin_for_pid(pid)
            .or_else(|| crate::wayland::sway_ipc::window_origin_for_pid(pid));
    }
    // Resolve a usable window xid. `xid == 0` means "no hint" (get_element_bounds
    // has no window context); fall back to this pid's first window — the same
    // convention resolve_element_local_coords uses. Guard the 0 case explicitly:
    // x11_window_origin(0) would resolve the *root* window to (0,0), not None.
    let win_xid = if xid != 0 {
        xid
    } else {
        crate::x11::list_windows(Some(pid)).first().map(|w| w.xid)?
    };
    // `?` here is the GTK gate: no _GTK_FRAME_EXTENTS → non-GTK toolkit → keep
    // the legacy Screen path (which those toolkits report correctly).
    let (fl, ft) = gtk_frame_extents(win_xid)?;
    let (ox, oy) = x11_window_origin(win_xid)?;
    Some((ox + fl, oy + ft))
}

fn screen_extent_rebase(
    x11_origin: (i32, i32),
    accessible_frame_origin: (i32, i32),
) -> Option<(i32, i32)> {
    // Chromium's broken "Screen" provider is rooted at the renderer-local
    // origin. A legitimate screen provider may differ from the X11 client
    // origin by title-bar/CSD extents; rebasing that small decoration delta
    // would move otherwise-correct GTK coordinates off their controls.
    if accessible_frame_origin.0.abs() <= 2 && accessible_frame_origin.1.abs() <= 2 {
        Some((
            x11_origin.0 - accessible_frame_origin.0,
            x11_origin.1 - accessible_frame_origin.1,
        ))
    } else {
        None
    }
}

/// Screen-coordinate bounds for the exact visited sequence rendered into the
/// current snapshot. Nodes without a usable Component interface, or whose
/// extents query fails/times out, are omitted rather than borrowing another
/// live traversal's ordinal.
///
/// GTK4 caveat: GTK4's AT-SPI bridge returns `GetExtents(Screen)` as `(0,0)`
/// for every element (issue #1564 / the #1739 a11y rework), so a screen query
/// is useless. Instead we query `CoordType::Window` (which GTK4 *does* report
/// correctly, per-widget) and add a deterministic screen offset — the X11
/// window origin plus the GTK4 CSD shadow inset from `_GTK_FRAME_EXTENTS` (see
/// [`window_to_screen_offset`]). For GTK3/Qt the inset is absent, so the
/// offset is just the X11 origin and the result matches the old screen path.
///
/// Returns `(element_index, x, y, width, height)` tuples.
async fn element_bounds_for_visited(
    visited: &[Visited<'_>],
    pid: u32,
    xid: u64,
) -> Vec<(usize, i32, i32, u32, u32)> {
    // Query WINDOW-relative extents and add a deterministic screen offset
    // (X11 window origin + GTK4 CSD inset). This fixes GTK4 — whose
    // CoordType::Screen reports every element at (0,0) — by using the
    // distinct per-widget WINDOW coords instead. On Wayland / when no X11
    // window resolves, `offset` is None and we keep the legacy Screen path
    // so non-X11 behaviour is unchanged.
    let offset = window_to_screen_offset(pid, xid);
    let coord = if offset.is_some() {
        CoordType::Window
    } else {
        CoordType::Screen
    };
    // Chromium on X11 labels its component extents as Screen while
    // returning coordinates relative to the renderer frame. Rebase
    // those values by comparing the top-level accessible frame with
    // the actual X11 window origin. Correct screen-coordinate providers
    // produce a zero delta; Chromium's local (0,0) frame produces the
    // required window-origin delta. GTK's explicit Window-coordinate
    // path above remains authoritative when available.
    let screen_rebase = if offset.is_none() && !crate::wayland::is_wayland() && xid != 0 {
        let x11_origin = x11_window_origin(xid);
        let frame = visited.iter().find(|node| {
            node.has_component
                && matches!(
                    node.role.to_ascii_lowercase().as_str(),
                    "frame" | "window" | "dialog" | "alert" | "file chooser"
                )
        });
        if let (Some(origin), Some(frame)) = (x11_origin, frame) {
            let accessible_origin = match call(frame.acc.proxies()).await {
                Some(Ok(proxies)) => match call(proxies.component()).await {
                    Some(Ok(component)) => {
                        match call(component.get_extents(CoordType::Screen)).await {
                            Some(Ok((x, y, _, _))) => Some((x, y)),
                            _ => None,
                        }
                    }
                    _ => None,
                },
                _ => None,
            };
            accessible_origin.and_then(|frame_origin| screen_extent_rebase(origin, frame_origin))
        } else {
            None
        }
    } else {
        None
    };
    // Renderer bridges can expose Window coordinates relative to an internal
    // frame whose origin is not (0,0) (Chromium commonly reports a negative
    // title-bar offset). Normalize that frame to the compositor window origin
    // before adding the screen offset. Native GTK reports (0,0), so this is a
    // no-op there.
    let window_frame_origin = if offset.is_some() {
        let frame = visited.iter().find(|node| {
            node.has_component
                && matches!(
                    node.role.to_ascii_lowercase().as_str(),
                    "frame" | "window" | "dialog" | "alert" | "file chooser"
                )
        });
        if let Some(frame) = frame {
            match call(frame.acc.proxies()).await {
                Some(Ok(proxies)) => match call(proxies.component()).await {
                    Some(Ok(component)) => {
                        match call(component.get_extents(CoordType::Window)).await {
                            Some(Ok((x, y, _, _))) => Some((x, y)),
                            _ => None,
                        }
                    }
                    _ => None,
                },
                _ => None,
            }
        } else {
            None
        }
    } else {
        None
    };
    let (mut offset_x, mut offset_y) = offset.or(screen_rebase).unwrap_or((0, 0));
    if let Some((frame_x, frame_y)) = window_frame_origin {
        offset_x = offset_x.saturating_sub(frame_x);
        offset_y = offset_y.saturating_sub(frame_y);
    }
    if let Some((ox, oy)) = offset {
        dlog!("element bounds: WINDOW coords + screen offset ({ox},{oy})");
    } else if let Some((ox, oy)) = screen_rebase {
        dlog!("element bounds: SCREEN coords + X11 frame rebase ({ox},{oy})");
    }

    let action_nodes: Vec<&Visited> = visited.iter().filter(|v| is_indexable(v)).collect();
    // Hard wall-clock budget for the whole collection: on pathological
    // trees individual D-Bus calls each burn up to CALL_TIMEOUT (geany's
    // unrealized nodes did exactly that). Return whatever was collected
    // in time, but do not impose an index-based node cap: a cap silently
    // stripped frames from valid controls later in renderer trees and
    // made PX targeting depend on DOM order.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut out = Vec::with_capacity(action_nodes.len());
    for (idx, node) in action_nodes.iter().enumerate() {
        if std::time::Instant::now() >= deadline {
            dlog!(
                "snapshot bounds: 20s budget exhausted at node {idx}; returning {} bound(s)",
                out.len()
            );
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
        if let Some(Ok((x, y, w, h))) = call(comp.get_extents(coord)).await {
            // Unrealized widgets (e.g. items inside closed menus/popovers)
            // report GetExtents as the i32::MIN sentinel and/or a degenerate
            // 0x0 / 1x1 size. Emitting those poisons downstream consumers
            // (overlay renderers, click targeting), so keep only elements
            // with plausible on-screen geometry. (Validate the raw extents,
            // before applying the screen offset, so the sentinel check still
            // catches unrealized widgets.)
            if x == i32::MIN || y == i32::MIN || x < -16384 || y < -16384 || w <= 1 || h <= 1 {
                continue;
            }
            out.push((idx, x + offset_x, y + offset_y, w as u32, h as u32));
        }
    }
    out
}

#[cfg(test)]
mod coord_tests {
    use super::parse_gtk_frame_extents;
    use super::{
        is_indexable_capabilities, is_passive_role, screen_extent_rebase, select_click_target,
    };

    #[test]
    fn editable_only_nodes_are_addressable() {
        assert!(is_indexable_capabilities(false, true, false));
        assert!(is_indexable_capabilities(true, false, false));
        assert!(is_indexable_capabilities(false, false, true));
        assert!(!is_indexable_capabilities(false, false, false));
    }

    #[test]
    fn screen_extents_are_rebased_from_accessible_frame_to_x11_origin() {
        assert_eq!(screen_extent_rebase((604, 80), (0, 0)), Some((604, 80)));
        assert_eq!(screen_extent_rebase((604, 100), (604, 80)), None);
        assert_eq!(screen_extent_rebase((604, 80), (604, 80)), None);
    }

    #[test]
    fn click_target_prefers_button_over_its_inner_label() {
        // The exact live GTK4 gnome-calculator case this fixes: button "7"
        // (idx 6, role 'button', 82,331 64x44) wraps a slightly smaller inner
        // label (idx 7, role 'label', 82,331 56x40). A click at the shared
        // center must actuate the BUTTON (idx 6) — area alone would pick the
        // smaller inert label (idx 7) → silent no-op "false success".
        let frames = vec![
            (6usize, 82, 331, 64, 44, false), // button "7"
            (7usize, 82, 331, 56, 40, true),  // inner label "7"
        ];
        assert_eq!(select_click_target(&frames, 114, 353), Some(6));
    }

    #[test]
    fn click_target_smallest_active_over_enclosing_panel() {
        let frames = vec![
            (0usize, 0, 0, 400, 600, false),   // panel
            (3usize, 80, 320, 64, 44, false),  // button "7"
            (5usize, 150, 320, 64, 44, false), // button "8"
        ];
        assert_eq!(select_click_target(&frames, 100, 340), Some(3));
        assert_eq!(select_click_target(&frames, 180, 340), Some(5));
    }

    #[test]
    fn click_target_falls_back_to_label_when_no_actuator_covers() {
        // A lone clickable label (no button covers the point) is still a valid
        // last-resort target — don't drop the click entirely.
        let frames = vec![(9usize, 10, 10, 30, 20, true)];
        assert_eq!(select_click_target(&frames, 20, 15), Some(9));
    }

    #[test]
    fn click_target_edges_exclusive_and_misses_return_none() {
        let frames = vec![(7usize, 10, 10, 20, 20, false)];
        assert_eq!(select_click_target(&frames, 10, 10), Some(7)); // top-left inclusive
        assert_eq!(select_click_target(&frames, 29, 29), Some(7)); // inside
        assert_eq!(select_click_target(&frames, 30, 20), None); // right edge exclusive
        assert_eq!(select_click_target(&frames, 20, 30), None); // bottom edge exclusive
        assert_eq!(select_click_target(&frames, 5, 5), None); // outside
        assert_eq!(select_click_target(&[], 0, 0), None); // no frames
    }

    #[test]
    fn passive_roles_classified() {
        assert!(is_passive_role("label"));
        assert!(is_passive_role("static text"));
        assert!(!is_passive_role("button"));
        assert!(!is_passive_role("push button"));
        assert!(!is_passive_role("text box")); // editable display is a real target
    }

    #[test]
    fn frame_extents_maps_left_and_top_not_right_or_bottom() {
        // _GTK_FRAME_EXTENTS = [left, right, top, bottom]; we need (left, top).
        assert_eq!(parse_gtk_frame_extents(&[61, 61, 55, 67]), Some((61, 55)));
        // Asymmetric values prove we don't accidentally read right([1])/bottom([3]).
        assert_eq!(parse_gtk_frame_extents(&[10, 20, 30, 40]), Some((10, 30)));
        // Maximized GTK4 window: zero inset, but property present.
        assert_eq!(parse_gtk_frame_extents(&[0, 0, 0, 0]), Some((0, 0)));
    }

    #[test]
    fn frame_extents_absent_or_short_is_none() {
        assert_eq!(parse_gtk_frame_extents(&[]), None);
        assert_eq!(parse_gtk_frame_extents(&[61, 61]), None);
        assert_eq!(parse_gtk_frame_extents(&[61, 61, 55]), None);
    }

    #[test]
    fn screen_reconstruction_matches_live_gnome_calculator() {
        // Regression anchor for the whole GTK4 fix, from a live-verified capture:
        // gnome-calculator button "7" = x11_window_origin (55,27)
        //   + _GTK_FRAME_EXTENTS inset (61,55) + atspi WINDOW coords (16,293)
        //   = screen (132,375).
        let (fl, ft) = parse_gtk_frame_extents(&[61, 61, 55, 67]).unwrap();
        let origin = (55, 27); // x11_window_origin
        let window = (16, 293); // atspi CoordType::Window
        let offset = (origin.0 + fl, origin.1 + ft); // window_to_screen_offset
        let screen = (offset.0 + window.0, offset.1 + window.1);
        assert_eq!(screen, (132, 375));
    }
}
