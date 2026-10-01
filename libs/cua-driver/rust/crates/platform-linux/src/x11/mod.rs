//! X11 window enumeration via x11rb.
//!
//! Uses _NET_CLIENT_LIST_STACKING to get the list of top-level windows,
//! then reads WM_NAME/_NET_WM_NAME, _NET_WM_PID, and geometry per window.
//! A toplevel without `_NET_WM_PID` is attributed through the X-Resource
//! extension when the server can prove it is a local client (see
//! [`crate::x11_client_pid`]); otherwise its `pid` stays `None`.

use anyhow::Result;
use x11rb::connection::Connection;
use x11rb::protocol::res::{ClientIdMask, ClientIdSpec, ConnectionExt as _};
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

#[derive(Debug, Clone)]
pub struct WindowInfo {
    /// X11 Window (XID) cast to u64.
    pub xid: u64,
    pub pid: Option<u32>,
    pub app_name: String,
    pub title: String,
    pub is_on_screen: bool,
    pub z_index: Option<usize>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The window the window manager reports as active (`_NET_ACTIVE_WINDOW`
/// on the root), if any.
pub fn active_window() -> Option<u64> {
    let (conn, screen_num) = RustConnection::connect(None).ok()?;
    let root = conn.setup().roots[screen_num].root;
    let atom = conn
        .intern_atom(true, b"_NET_ACTIVE_WINDOW")
        .ok()?
        .reply()
        .ok()?
        .atom;
    if atom == 0 {
        return None;
    }
    let reply = conn
        .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    let id = reply.value32()?.next()?;
    (id != 0).then_some(u64::from(id))
}

/// List top-level windows, optionally filtered by pid.
pub fn list_windows(filter_pid: Option<u32>) -> Vec<WindowInfo> {
    match list_windows_inner(filter_pid) {
        Ok(w) => w,
        Err(_) => Vec::new(),
    }
}

/// Whether the X server still knows a window by this id.
pub fn window_exists(xid: u64) -> bool {
    let Ok(xid) = u32::try_from(xid) else {
        return false;
    };
    let Ok((conn, _)) = RustConnection::connect(None) else {
        return false;
    };
    conn.get_window_attributes(xid)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_some()
}

/// Verify that an X11 window still exists and belongs to the requested process.
///
/// Checking `/proc/<pid>` alone is insufficient because Linux may recycle the
/// PID after the original application exits. The XID owner binds the two parts
/// of a `get_window_state` target and fails closed when either is stale.
pub fn window_belongs_to_pid(xid: u64, pid: u32) -> bool {
    let Ok(xid) = u32::try_from(xid) else {
        return false;
    };
    let Ok((conn, _)) = RustConnection::connect(None) else {
        return false;
    };
    window_owner_matches(OwnerResolver::new(&conn).owner_pid(xid), pid)
}

fn window_owner_matches(owner: Option<u32>, requested_pid: u32) -> bool {
    owner == Some(requested_pid)
}

fn list_windows_inner(filter_pid: Option<u32>) -> Result<Vec<WindowInfo>> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    // Get _NET_CLIENT_LIST_STACKING (or fallback to _NET_CLIENT_LIST).
    let windows = get_window_list(&conn, root)?;

    let owners = OwnerResolver::new(&conn);
    let mut result = Vec::new();
    for (z_index, xid) in windows.into_iter().enumerate() {
        let pid = owners.owner_pid(xid);
        if let Some(fp) = filter_pid {
            if pid != Some(fp) {
                continue;
            }
        }

        let title = get_window_title(&conn, xid).unwrap_or_default();
        if title.trim().is_empty() {
            continue;
        }
        let app_name = get_window_class(&conn, xid)
            .map(|(instance, class)| if class.is_empty() { instance } else { class })
            .unwrap_or_default();
        let is_on_screen = conn
            .get_window_attributes(xid)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|attributes| attributes.map_state == MapState::VIEWABLE);

        let geom = conn.get_geometry(xid)?.reply().ok();
        let (x, y, w, h) = if let Some(g) = geom {
            // Translate to root coordinates.
            let trans = conn.translate_coordinates(xid, root, 0, 0)?.reply().ok();
            let (rx, ry) = trans
                .map(|t| (t.dst_x as i32, t.dst_y as i32))
                .unwrap_or((0, 0));
            (rx, ry, g.width as u32, g.height as u32)
        } else {
            (0, 0, 0, 0)
        };

        result.push(WindowInfo {
            xid: xid as u64,
            pid,
            app_name,
            title,
            is_on_screen,
            // EWMH stacking lists are bottom-to-top, so the enumeration index
            // already follows the shared "higher z_index is frontmost" contract.
            z_index: Some(z_index),
            x,
            y,
            width: w,
            height: h,
        });
    }

    Ok(result)
}

fn get_window_list(conn: &RustConnection, root: Window) -> Result<Vec<Window>> {
    let atom_names = ["_NET_CLIENT_LIST_STACKING", "_NET_CLIENT_LIST"];
    for name in &atom_names {
        if let Ok(atom) = get_atom(conn, name) {
            if let Ok(reply) = conn
                .get_property(false, root, atom, AtomEnum::WINDOW, 0, u32::MAX)?
                .reply()
            {
                let windows: Vec<Window> = reply
                    .value32()
                    .map(|iter| iter.collect())
                    .unwrap_or_default();
                if client_list_property(reply.type_, windows.as_slice()).is_some() {
                    return Ok(windows);
                }
            }
        }
    }

    // No EWMH client-list property means there may be no window manager. In
    // that case only expose mapped root children; unmapped Electron children
    // can otherwise be reported before a late-starting WM reparents them.
    let tree = conn.query_tree(root)?.reply()?;
    Ok(tree
        .children
        .into_iter()
        .filter(|window| {
            conn.get_window_attributes(*window)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|attributes| fallback_window_is_listable(attributes.map_state))
                .unwrap_or(false)
        })
        .collect())
}

fn client_list_property(property_type: Atom, windows: &[Window]) -> Option<&[Window]> {
    (property_type != x11rb::NONE).then_some(windows)
}

fn fallback_window_is_listable(map_state: MapState) -> bool {
    map_state == MapState::VIEWABLE
}

fn get_atom(conn: &RustConnection, name: &str) -> Result<Atom> {
    Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

/// Ask the X11 window manager to set one exact top-level window frame, then
/// read the window geometry back in the same desktop coordinate space exposed
/// by `list_windows`. Uses EWMH rather than configuring a client directly.
pub fn set_window_frame(
    xid: u64,
    pid: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(Option<WindowInfo>, bool, Option<String>)> {
    let xid = u32::try_from(xid).map_err(|_| anyhow::anyhow!("window_id is out of X11 range"))?;
    let (conn, screen_num) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    match OwnerResolver::new(&conn).owner_pid(xid) {
        Some(owner) if owner == pid => {}
        Some(owner) => anyhow::bail!("window_id {xid} belongs to pid {owner}, not pid {pid}"),
        None => anyhow::bail!("window_id {xid} has no verifiable owner pid ({UNATTRIBUTED_OWNER})"),
    }

    let atom = get_atom(&conn, "_NET_MOVERESIZE_WINDOW")?;
    let fields = moveresize_window_flags();
    let source_indication = 1_u32 << 12; // normal application (EWMH §4.1.5)
    let event = ClientMessageEvent::new(
        32,
        xid,
        atom,
        ClientMessageData::from([
            fields | source_indication,
            x as u32,
            y as u32,
            width,
            height,
        ]),
    );
    let mutation_error = conn
        .send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )
        .and_then(|_| conn.flush())
        .err()
        .map(|error| format!("_NET_MOVERESIZE_WINDOW request failed: {error}"));

    let requested = (x, y, width, height);
    let mut observed = None;
    for _ in 0..8 {
        observed = list_windows(Some(pid))
            .into_iter()
            .find(|window| window.xid == u64::from(xid));
        if observed
            .as_ref()
            .is_some_and(|window| (window.x, window.y, window.width, window.height) == requested)
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    let confirmed = observed
        .as_ref()
        .is_some_and(|window| (window.x, window.y, window.width, window.height) == requested);
    Ok((observed, confirmed, mutation_error))
}

/// EWMH §4.2 requires an explicit StaticGravity when a pager-style client
/// wants the requested geometry to include server-side window decorations.
/// A zero gravity delegates to WM_NORMAL_HINTS and makes the same request land
/// at different client offsets under window managers such as Xfwm.
fn moveresize_window_flags() -> u32 {
    const STATIC_GRAVITY: u32 = 10;
    const X_PRESENT: u32 = 1 << 8;
    const Y_PRESENT: u32 = 1 << 9;
    const WIDTH_PRESENT: u32 = 1 << 10;
    const HEIGHT_PRESENT: u32 = 1 << 11;
    STATIC_GRAVITY | X_PRESENT | Y_PRESENT | WIDTH_PRESENT | HEIGHT_PRESENT
}

/// The listed toplevel that holds the core keyboard focus (`XGetInputFocus`),
/// if any: the focus usually sits on a child of the client window, so the
/// ancestors are walked until one of `candidates` is met.
pub fn focused_window_among(candidates: &[u64]) -> Option<u64> {
    let (conn, screen_num) = RustConnection::connect(None).ok()?;
    let root = conn.setup().roots[screen_num].root;
    let focus = conn.get_input_focus().ok()?.reply().ok()?.focus;
    if focus <= 1 {
        return None;
    }
    let mut current = focus;
    for _ in 0..32 {
        if candidates.contains(&u64::from(current)) {
            return Some(u64::from(current));
        }
        let tree = conn.query_tree(current).ok()?.reply().ok()?;
        if tree.parent == 0 || tree.parent == root {
            return None;
        }
        current = tree.parent;
    }
    None
}

/// `WM_TRANSIENT_FOR` of a toplevel: the window it is a dialog of. `None`
/// when unset or pointing at the root (group-transient utility windows).
pub fn transient_for(xid: u64) -> Option<u64> {
    let xid = u32::try_from(xid).ok()?;
    let (conn, screen_num) = RustConnection::connect(None).ok()?;
    let root = conn.setup().roots[screen_num].root;
    let reply = conn
        .get_property(
            false,
            xid,
            AtomEnum::WM_TRANSIENT_FOR,
            AtomEnum::WINDOW,
            0,
            1,
        )
        .ok()?
        .reply()
        .ok()?;
    let owner = reply.value32()?.next()?;
    (owner != 0 && owner != root).then_some(u64::from(owner))
}

/// Windows NOT owned by `pid` whose `WM_TRANSIENT_FOR` chain (bounded to
/// avoid cycles) resolves to one of `pid`'s own top-level windows.
///
/// A dialog or plugin window can legitimately run as a *different* process
/// than the application it belongs to — e.g. GIMP's separate-process export
/// option dialogs, or LibreOffice's "Document Recovery" dialog surfacing
/// under a distinct `soffice.bin` instance. Exact-pid matching alone makes
/// `get_window_state`'s `dialogs[]` invisible to such windows even though
/// they are clearly the target application's own popup.
///
/// `WM_TRANSIENT_FOR` pointing (directly or transitively) at a window this
/// pid actually owns is used as the sole correlation signal because it is an
/// explicit, spoofing-resistant relationship the window manager enforces —
/// unlike `WM_CLASS`/process-name similarity, which many unrelated
/// applications built on the same toolkit share and which a hostile window
/// could set to anything.
pub fn list_cross_pid_transient_windows(pid: u32) -> Vec<WindowInfo> {
    list_cross_pid_transient_windows_with(pid, list_windows(None), transient_for)
}

fn list_cross_pid_transient_windows_with(
    pid: u32,
    all_windows: Vec<WindowInfo>,
    transient_for: impl Fn(u64) -> Option<u64>,
) -> Vec<WindowInfo> {
    let own_xids: std::collections::HashSet<u64> = all_windows
        .iter()
        .filter(|w| w.pid == Some(pid))
        .map(|w| w.xid)
        .collect();
    if own_xids.is_empty() {
        return Vec::new();
    }
    all_windows
        .into_iter()
        .filter(|w| w.pid != Some(pid))
        .filter(|w| transient_chain_reaches(w.xid, &own_xids, &transient_for))
        .collect()
}

/// Walks `WM_TRANSIENT_FOR` from `start`, bounded to guard against a cycle a
/// misbehaving client could create, until it lands on a member of `targets`.
fn transient_chain_reaches(
    start: u64,
    targets: &std::collections::HashSet<u64>,
    transient_for: &impl Fn(u64) -> Option<u64>,
) -> bool {
    let mut current = start;
    for _ in 0..8 {
        match transient_for(current) {
            Some(owner) if targets.contains(&owner) => return true,
            Some(owner) if owner == current => return false,
            Some(owner) => current = owner,
            None => return false,
        }
    }
    false
}

/// The window a pid-only keyboard action means in a multi-window app, in
/// order: the pid's window holding the core focus; its topmost on-screen
/// transient dialog (a file chooser, a filter dialog); the WM's active
/// window when it is the pid's; the largest mapped toplevel. `transient_for`
/// answers `WM_TRANSIENT_FOR` for a window id.
pub fn pick_pid_window(
    windows: &[WindowInfo],
    focused: Option<u64>,
    transient_for: impl Fn(u64) -> Option<u64>,
    active: Option<u64>,
) -> Option<u64> {
    if let Some(focused) = focused.filter(|f| windows.iter().any(|w| w.xid == *f)) {
        return Some(focused);
    }
    let on_screen: Vec<&WindowInfo> = windows.iter().filter(|w| w.is_on_screen).collect();
    if let Some(dialog) = on_screen
        .iter()
        .filter(|w| w.width > 0 && w.height > 0 && transient_for(w.xid).is_some())
        .max_by_key(|w| w.z_index.unwrap_or(0))
    {
        return Some(dialog.xid);
    }
    if let Some(active) = active.filter(|a| windows.iter().any(|w| w.xid == *a)) {
        return Some(active);
    }
    on_screen
        .iter()
        .max_by_key(|w| {
            (
                u64::from(w.width) * u64::from(w.height),
                w.z_index.unwrap_or(0),
            )
        })
        .map(|w| w.xid)
}

/// Geometry, title and owner of ANY mapped X window (an override-redirect
/// popup menu included), unlike [`list_windows`], which only enumerates the
/// WM's client list. `None` when the window does not exist.
pub fn window_info(xid: u64) -> Option<WindowInfo> {
    let window = u32::try_from(xid).ok()?;
    let (conn, screen_num) = RustConnection::connect(None).ok()?;
    let root = conn.setup().roots[screen_num].root;
    let attributes = conn.get_window_attributes(window).ok()?.reply().ok()?;
    let geom = conn.get_geometry(window).ok()?.reply().ok()?;
    let trans = conn
        .translate_coordinates(window, root, 0, 0)
        .ok()?
        .reply()
        .ok()?;
    let pid = get_window_pid(&conn, window).ok().flatten();
    let title = get_window_title(&conn, window).unwrap_or_default();
    let app_name = get_window_class(&conn, window)
        .map(|(instance, class)| if class.is_empty() { instance } else { class })
        .unwrap_or_default();
    Some(WindowInfo {
        xid,
        pid,
        app_name,
        title,
        is_on_screen: attributes.map_state == MapState::VIEWABLE,
        z_index: None,
        x: i32::from(trans.dst_x),
        y: i32::from(trans.dst_y),
        width: u32::from(geom.width),
        height: u32::from(geom.height),
    })
}

/// Ask the window manager to close `xid` (EWMH `_NET_CLOSE_WINDOW`, which
/// the WM turns into `WM_DELETE_WINDOW` for a cooperating client): what
/// Alt+F4 does through mutter's passive grab, which a virtual keyboard cannot
/// reach. Only a window of `pid` is accepted.
pub fn close_window(xid: u64, pid: u32) -> Result<()> {
    let window =
        u32::try_from(xid).map_err(|_| anyhow::anyhow!("window_id is out of X11 range"))?;
    let (conn, screen_num) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    match OwnerResolver::new(&conn).owner_pid(window) {
        Some(owner) if owner == pid => {}
        Some(owner) => anyhow::bail!("window_id {xid} belongs to pid {owner}, not pid {pid}"),
        None => anyhow::bail!("window_id {xid} has no verifiable owner pid ({UNATTRIBUTED_OWNER})"),
    }
    let atom = get_atom(&conn, "_NET_CLOSE_WINDOW")?;
    let event = ClientMessageEvent::new(
        32,
        window,
        atom,
        ClientMessageData::from([0u32, 1u32, 0, 0, 0]),
    );
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )?;
    conn.flush()?;
    Ok(())
}

/// `_NET_WM_STATE` carries `_NET_WM_STATE_MODAL`: the dialog blocks input to
/// the window it is transient for.
pub fn window_is_modal(xid: u64) -> bool {
    let Ok(xid) = u32::try_from(xid) else {
        return false;
    };
    let Ok((conn, _)) = RustConnection::connect(None) else {
        return false;
    };
    let (Ok(state_atom), Ok(modal_atom)) = (
        get_atom(&conn, "_NET_WM_STATE"),
        get_atom(&conn, "_NET_WM_STATE_MODAL"),
    ) else {
        return false;
    };
    conn.get_property(false, xid, state_atom, AtomEnum::ATOM, 0, 64)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| reply.value32().map(|atoms| atoms.collect::<Vec<_>>()))
        .is_some_and(|atoms| atoms.contains(&modal_atom))
}

/// True while `xid` exists on the server and is viewable.
pub fn window_is_viewable(xid: u64) -> bool {
    window_info(xid).is_some_and(|w| w.is_on_screen)
}

/// `_NET_WM_PID` of a window, when it advertises one.
///
/// Deliberately property-only: input paths walk this up through child and
/// window-manager frame windows, where an X-Resource answer would name the
/// window manager. Client-list toplevels use [`OwnerResolver`] instead.
pub fn window_pid(xid: u64) -> Option<u32> {
    let xid = u32::try_from(xid).ok()?;
    let (conn, _) = RustConnection::connect(None).ok()?;
    get_window_pid(&conn, xid).ok().flatten()
}

fn get_window_pid(conn: &RustConnection, window: Window) -> Result<Option<u32>> {
    let atom = get_atom(conn, "_NET_WM_PID")?;
    let reply = conn
        .get_property(false, window, atom, AtomEnum::CARDINAL, 0, 1)?
        .reply()?;
    Ok(reply.value32().and_then(|mut i| i.next()))
}

/// Why an X11 toplevel can be listed with no owner pid.
const UNATTRIBUTED_OWNER: &str = "the client publishes no _NET_WM_PID and the X server's \
X-Resource extension could not attribute it to a local process: XRes 1.2 is unavailable, \
the client is remote or forwarded, or the server is outside this PID namespace";

/// Owner pid of client-list toplevels: `_NET_WM_PID` when published, else
/// the X-Resource `LocalClientPID` of the connection that created the window,
/// subject to the fail-closed rules in [`crate::x11_client_pid`]. The XRes
/// probe runs at most once per connection and only for windows that lack
/// `_NET_WM_PID`.
struct OwnerResolver<'c> {
    conn: &'c RustConnection,
    xres: std::cell::OnceCell<bool>,
    hostname: std::cell::OnceCell<Option<String>>,
}

impl<'c> OwnerResolver<'c> {
    fn new(conn: &'c RustConnection) -> Self {
        Self {
            conn,
            xres: std::cell::OnceCell::new(),
            hostname: std::cell::OnceCell::new(),
        }
    }

    fn owner_pid(&self, window: Window) -> Option<u32> {
        match get_window_pid(self.conn, window) {
            Ok(Some(pid)) => Some(pid),
            Ok(None) => self.xres_pid(window),
            Err(_) => None,
        }
    }

    fn xres_pid(&self, window: Window) -> Option<u32> {
        use crate::x11_client_pid::{
            client_base, client_machine_is_local, select_owner_pid, xres_supports_client_ids,
            XresClientValue,
        };
        let available = *self.xres.get_or_init(|| {
            self.conn
                .res_query_version(1, 2)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .is_some_and(|v| xres_supports_client_ids(v.server_major, v.server_minor))
        });
        if !available {
            return None;
        }
        let machine = get_client_machine(self.conn, window).ok()?;
        let hostname = self.hostname.get_or_init(|| {
            std::fs::read_to_string("/proc/sys/kernel/hostname")
                .ok()
                .map(|h| h.trim().to_owned())
        });
        if !client_machine_is_local(machine.as_deref(), hostname.as_deref()) {
            return None;
        }
        let setup = self.conn.setup();
        let own_base = setup.resource_id_base;
        let specs = [
            ClientIdSpec {
                client: own_base,
                mask: ClientIdMask::LOCAL_CLIENT_PID,
            },
            ClientIdSpec {
                client: window,
                mask: ClientIdMask::LOCAL_CLIENT_PID,
            },
        ];
        let reply = self.conn.res_query_client_ids(&specs).ok()?.reply().ok()?;
        let values: Vec<XresClientValue<'_>> = reply
            .ids
            .iter()
            .map(|id| XresClientValue {
                client: id.spec.client,
                mask: u32::from(id.spec.mask),
                value: &id.value,
            })
            .collect();
        let pid = select_owner_pid(
            &values,
            own_base,
            std::process::id(),
            client_base(window, setup.resource_id_mask),
        )?;
        // The server keys the query by the client slot in the XID. A window
        // that still exists after the reply proves its creator held that slot
        // when the server answered, so a recycled slot cannot be attributed.
        self.conn.get_window_attributes(window).ok()?.reply().ok()?;
        Some(pid)
    }
}

/// `WM_CLIENT_MACHINE`, `None` when unset. Errors when the window is gone.
fn get_client_machine(conn: &RustConnection, window: Window) -> Result<Option<String>> {
    let reply = conn
        .get_property(
            false,
            window,
            AtomEnum::WM_CLIENT_MACHINE,
            AtomEnum::ANY,
            0,
            256,
        )?
        .reply()?;
    if reply.type_ == x11rb::NONE {
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&reply.value);
    Ok(Some(value.trim_end_matches('\0').to_owned()))
}

fn get_window_title(conn: &RustConnection, window: Window) -> Result<String> {
    // Try _NET_WM_NAME (UTF-8) first.
    if let Ok(atom) = get_atom(conn, "_NET_WM_NAME") {
        if let Ok(utf8_atom) = get_atom(conn, "UTF8_STRING") {
            if let Ok(reply) = conn
                .get_property(false, window, atom, utf8_atom, 0, 1024)?
                .reply()
            {
                if !reply.value.is_empty() {
                    return Ok(String::from_utf8_lossy(&reply.value).into_owned());
                }
            }
        }
    }
    // Fallback: WM_NAME (latin-1 / ASCII).
    let reply = conn
        .get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)?
        .reply()?;
    Ok(String::from_utf8_lossy(&reply.value).into_owned())
}

/// Return the WM_CLASS pair for `xid` as `(instance, class)`.
///
/// X11's `WM_CLASS` property is two NUL-separated strings; the first is
/// the instance name, the second is the class name. Either field can
/// be empty. Used by [`crate::terminal::is_terminal_window`] to detect
/// terminal emulators that share a process tree with another GUI
/// (e.g. Ghostty's `WM_CLASS = "ghostty\0Ghostty\0"`).
///
/// Returns `None` when no X connection is available, the window has no
/// WM_CLASS atom set, or the property could not be read.
pub fn wm_class_for_window(xid: u64) -> Option<(String, String)> {
    let (conn, _) = RustConnection::connect(None).ok()?;
    get_window_class(&conn, xid as u32)
}

pub(crate) fn get_window_class(conn: &RustConnection, xid: Window) -> Option<(String, String)> {
    let reply = conn
        .get_property(
            false,
            xid as u32,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            0,
            512,
        )
        .ok()?
        .reply()
        .ok()?;
    // Two NUL-terminated strings, instance then class; either may be empty
    // (`\0Foo\0`), so the position decides which is which.
    let raw = reply.value;
    let mut parts = raw.split(|&b| b == 0);
    let instance = parts
        .next()
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .unwrap_or_default();
    let class = parts
        .next()
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .unwrap_or_default();
    if instance.is_empty() && class.is_empty() {
        return None;
    }
    Some((instance, class))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_present_client_list_does_not_fall_back_to_query_tree() {
        assert_eq!(client_list_property(1, &[]), Some([].as_slice()));
    }

    #[test]
    fn absent_client_list_allows_query_tree_fallback() {
        assert_eq!(client_list_property(x11rb::NONE, &[]), None);
    }

    #[test]
    fn stale_or_reused_pid_window_owner_fails_closed() {
        assert!(window_owner_matches(Some(42), 42));
        assert!(!window_owner_matches(Some(43), 42));
        assert!(!window_owner_matches(None, 42));
    }

    #[test]
    fn query_tree_fallback_only_lists_viewable_windows() {
        assert!(fallback_window_is_listable(MapState::VIEWABLE));
        assert!(!fallback_window_is_listable(MapState::UNMAPPED));
        assert!(!fallback_window_is_listable(MapState::UNVIEWABLE));
    }

    #[test]
    fn moveresize_requests_static_gravity_and_all_frame_fields() {
        let flags = moveresize_window_flags();
        assert_eq!(flags & 0xff, 10);
        assert_eq!(flags & 0x0f00, 0x0f00);
        assert_eq!(flags & !0x0fff, 0);
    }

    fn win(xid: u64, pid: Option<u32>, title: &str) -> WindowInfo {
        WindowInfo {
            xid,
            pid,
            app_name: String::new(),
            title: title.into(),
            is_on_screen: true,
            z_index: None,
            x: 0,
            y: 0,
            width: 400,
            height: 300,
        }
    }

    #[test]
    fn a_different_process_dialog_transient_to_the_target_pid_is_surfaced() {
        // GIMP-style separate-process export dialog: xid 90 belongs to pid 999
        // (not the target pid 7) but is WM_TRANSIENT_FOR the target's own
        // window (10).
        let windows = vec![
            win(10, Some(7), "GIMP"),
            win(90, Some(999), "Export Image as JPEG"),
        ];
        let cross = list_cross_pid_transient_windows_with(7, windows, |xid| match xid {
            90 => Some(10),
            _ => None,
        });
        assert_eq!(cross.len(), 1);
        assert_eq!(cross[0].xid, 90);
        assert_eq!(cross[0].pid, Some(999));
    }

    #[test]
    fn a_transitive_transient_chain_through_another_cross_pid_window_still_resolves() {
        // xid 91 is transient-for xid 90, which is transient-for the target's
        // own window 10: both 90 and 91 should be attributed to pid 7.
        let windows = vec![
            win(10, Some(7), "LibreOffice"),
            win(90, Some(999), "Recovery helper"),
            win(91, Some(999), "Document Recovery"),
        ];
        let cross = list_cross_pid_transient_windows_with(7, windows, |xid| match xid {
            90 => Some(10),
            91 => Some(90),
            _ => None,
        });
        let mut xids: Vec<u64> = cross.iter().map(|w| w.xid).collect();
        xids.sort();
        assert_eq!(xids, vec![90, 91]);
    }

    #[test]
    fn an_unrelated_window_of_a_totally_different_app_is_not_attributed() {
        // No transient_for relationship at all to the target's windows: this
        // must never be surfaced, no matter how similar its title/class.
        let windows = vec![win(10, Some(7), "GIMP"), win(50, Some(555), "Firefox")];
        let cross = list_cross_pid_transient_windows_with(7, windows, |_| None);
        assert!(cross.is_empty());
    }

    #[test]
    fn a_transient_cycle_fails_closed_instead_of_looping_forever() {
        let windows = vec![win(10, Some(7), "target"), win(90, Some(999), "cyclic")];
        let cross = list_cross_pid_transient_windows_with(7, windows, |xid| match xid {
            90 => Some(91),
            91 => Some(90),
            _ => None,
        });
        assert!(cross.is_empty());
    }

    #[test]
    fn no_own_windows_for_the_pid_yields_nothing_to_correlate_against() {
        let windows = vec![win(90, Some(999), "orphan dialog")];
        let cross =
            list_cross_pid_transient_windows_with(7, windows, |xid| (xid == 90).then_some(10));
        assert!(cross.is_empty());
    }
}
