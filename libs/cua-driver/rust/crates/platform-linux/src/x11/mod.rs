//! X11 window enumeration via x11rb.
//!
//! Uses _NET_CLIENT_LIST_STACKING to get the list of top-level windows,
//! then reads WM_NAME/_NET_WM_NAME, _NET_WM_PID, and geometry per window.

use anyhow::Result;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

#[derive(Debug, Clone)]
pub struct WindowInfo {
    /// X11 Window (XID) cast to u64.
    pub xid: u64,
    pub pid: Option<u32>,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// List top-level windows, optionally filtered by pid.
pub fn list_windows(filter_pid: Option<u32>) -> Vec<WindowInfo> {
    match list_windows_inner(filter_pid) {
        Ok(w) => w,
        Err(_) => Vec::new(),
    }
}

fn list_windows_inner(filter_pid: Option<u32>) -> Result<Vec<WindowInfo>> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    // Get _NET_CLIENT_LIST_STACKING (or fallback to _NET_CLIENT_LIST).
    let windows = get_window_list(&conn, root)?;

    let mut result = Vec::new();
    for xid in windows {
        let pid = get_window_pid(&conn, xid).ok().flatten();
        if let Some(fp) = filter_pid {
            if pid != Some(fp) { continue; }
        }

        let title = get_window_title(&conn, xid).unwrap_or_default();
        if title.trim().is_empty() { continue; }

        let geom = conn.get_geometry(xid)?.reply().ok();
        let (x, y, w, h) = if let Some(g) = geom {
            // Translate to root coordinates.
            let trans = conn.translate_coordinates(xid, root, 0, 0)?.reply().ok();
            let (rx, ry) = trans.map(|t| (t.dst_x as i32, t.dst_y as i32)).unwrap_or((0, 0));
            (rx, ry, g.width as u32, g.height as u32)
        } else {
            (0, 0, 0, 0)
        };

        result.push(WindowInfo { xid: xid as u64, pid, title, x, y, width: w, height: h });
    }

    Ok(result)
}

fn get_window_list(conn: &RustConnection, root: Window) -> Result<Vec<Window>> {
    let atom_names = ["_NET_CLIENT_LIST_STACKING", "_NET_CLIENT_LIST"];
    for name in &atom_names {
        if let Ok(atom) = get_atom(conn, name) {
            if let Ok(reply) = conn.get_property(false, root, atom, AtomEnum::WINDOW, 0, u32::MAX)?.reply() {
                let windows: Vec<Window> = reply.value32()
                    .map(|iter| iter.collect())
                    .unwrap_or_default();
                if !windows.is_empty() {
                    return Ok(windows);
                }
            }
        }
    }
    // Fallback: query tree from root.
    let tree = conn.query_tree(root)?.reply()?;
    Ok(tree.children)
}

fn get_atom(conn: &RustConnection, name: &str) -> Result<Atom> {
    Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

fn get_window_pid(conn: &RustConnection, window: Window) -> Result<Option<u32>> {
    let atom = get_atom(conn, "_NET_WM_PID")?;
    let reply = conn.get_property(false, window, atom, AtomEnum::CARDINAL, 0, 1)?.reply()?;
    Ok(reply.value32().and_then(|mut i| i.next()))
}

fn get_window_title(conn: &RustConnection, window: Window) -> Result<String> {
    // Try _NET_WM_NAME (UTF-8) first.
    if let Ok(atom) = get_atom(conn, "_NET_WM_NAME") {
        if let Ok(utf8_atom) = get_atom(conn, "UTF8_STRING") {
            if let Ok(reply) = conn.get_property(false, window, atom, utf8_atom, 0, 1024)?.reply() {
                if !reply.value.is_empty() {
                    return Ok(String::from_utf8_lossy(&reply.value).into_owned());
                }
            }
        }
    }
    // Fallback: WM_NAME (latin-1 / ASCII).
    let reply = conn.get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)?.reply()?;
    Ok(String::from_utf8_lossy(&reply.value).into_owned())
}

/// Get the currently active window (via _NET_ACTIVE_WINDOW).
pub fn get_active_window() -> Result<Option<u64>> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    let atom = get_atom(&conn, "_NET_ACTIVE_WINDOW")?;
    let reply = conn.get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)?.reply()?;
    Ok(reply.value32().and_then(|mut i| i.next()).map(|w| w as u64))
}

/// Activate (focus/raise) a window by sending a _NET_ACTIVE_WINDOW client message.
/// This is the EWMH-compliant way to request focus on a window.
pub fn activate_window(xid: u64) -> Result<()> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;
    let window = xid as Window;

    let atom = get_atom(&conn, "_NET_ACTIVE_WINDOW")?;

    // Build the ClientMessage event as per EWMH spec:
    // window = target window
    // message_type = _NET_ACTIVE_WINDOW
    // format = 32
    // data.data32[0] = source indication (1 = application, 2 = pager/user)
    // data.data32[1] = timestamp (0 = CurrentTime)
    // data.data32[2] = requestor's active window (0 = none)
    let event = ClientMessageEvent {
        response_type: CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window,
        type_: atom,
        data: [2, 0, 0, 0, 0].into(), // source=2 (user action), timestamp=0
    };

    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )?.check()?;
    conn.flush()?;

    Ok(())
}

/// Get the current desktop/workspace index (via _NET_CURRENT_DESKTOP).
pub fn get_current_desktop() -> Result<Option<u32>> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    let atom = get_atom(&conn, "_NET_CURRENT_DESKTOP")?;
    let reply = conn.get_property(false, root, atom, AtomEnum::CARDINAL, 0, 1)?.reply()?;
    Ok(reply.value32().and_then(|mut i| i.next()))
}

/// Get the total number of desktops/workspaces (via _NET_NUMBER_OF_DESKTOPS).
pub fn get_number_of_desktops() -> Result<Option<u32>> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    let atom = get_atom(&conn, "_NET_NUMBER_OF_DESKTOPS")?;
    let reply = conn.get_property(false, root, atom, AtomEnum::CARDINAL, 0, 1)?.reply()?;
    Ok(reply.value32().and_then(|mut i| i.next()))
}

/// Get the desktop/workspace a window is on (via _NET_WM_DESKTOP).
pub fn get_window_desktop(xid: u64) -> Result<Option<u32>> {
    let (conn, _screen_num) = RustConnection::connect(None)?;
    let window = xid as Window;

    let atom = get_atom(&conn, "_NET_WM_DESKTOP")?;
    let reply = conn.get_property(false, window, atom, AtomEnum::CARDINAL, 0, 1)?.reply()?;
    Ok(reply.value32().and_then(|mut i| i.next()))
}

/// Move a window to a specific desktop/workspace (via _NET_WM_DESKTOP ClientMessage).
pub fn set_window_desktop(xid: u64, desktop: u32) -> Result<()> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;
    let window = xid as Window;

    let atom = get_atom(&conn, "_NET_WM_DESKTOP")?;

    // EWMH spec: _NET_WM_DESKTOP ClientMessage
    // window = target window
    // message_type = _NET_WM_DESKTOP
    // format = 32
    // data.data32[0] = new desktop index
    // data.data32[1] = source indication (1 = application, 2 = pager)
    let event = ClientMessageEvent {
        response_type: CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window,
        type_: atom,
        data: [desktop, 2, 0, 0, 0].into(), // desktop, source=2 (pager/user action)
    };

    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )?.check()?;
    conn.flush()?;

    Ok(())
}
