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
    /// Normalized stacking position: 0 is backmost and larger values are
    /// closer to the front. None when the window source exposes no ordering.
    pub z_index: Option<usize>,
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

    let window_list = get_window_list(&conn, root)?;

    let mut result = Vec::new();
    for xid in window_list.windows {
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

        result.push(WindowInfo {
            xid: xid as u64, pid, title, x, y, width: w, height: h, z_index: None,
        });
    }

    // Both _NET_CLIENT_LIST_STACKING and QueryTree are bottom-to-top. Assign
    // after filtering so every response uses the cross-platform invariant:
    // 0 is backmost and larger values are closer to the front.
    if window_list.stacking_known {
        for (z_index, window) in result.iter_mut().enumerate() {
            window.z_index = Some(z_index);
        }
    }

    Ok(result)
}

struct WindowList {
    windows: Vec<Window>,
    stacking_known: bool,
}

fn get_window_list(conn: &RustConnection, root: Window) -> Result<WindowList> {
    if let Ok(atom) = get_atom(conn, "_NET_CLIENT_LIST_STACKING") {
        if let Ok(reply) = conn.get_property(false, root, atom, AtomEnum::WINDOW, 0, u32::MAX)?.reply() {
            let windows: Vec<Window> = reply.value32()
                .map(|iter| iter.collect())
                .unwrap_or_default();
            if !windows.is_empty() {
                return Ok(WindowList { windows, stacking_known: true });
            }
        }
    }

    // _NET_CLIENT_LIST is in initial mapping order, not stacking order.
    if let Ok(atom) = get_atom(conn, "_NET_CLIENT_LIST") {
        if let Ok(reply) = conn.get_property(false, root, atom, AtomEnum::WINDOW, 0, u32::MAX)?.reply() {
            let windows: Vec<Window> = reply.value32()
                .map(|iter| iter.collect())
                .unwrap_or_default();
            if !windows.is_empty() {
                return Ok(WindowList { windows, stacking_known: false });
            }
        }
    }

    // QueryTree returns children in bottom-to-top stacking order.
    let tree = conn.query_tree(root)?.reply()?;
    Ok(WindowList {
        windows: tree.children,
        stacking_known: true,
    })
}

fn get_atom(conn: &RustConnection, name: &str) -> Result<Atom> {
    Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

fn get_window_pid(conn: &RustConnection, window: Window) -> Result<Option<u32>> {
    let atom = get_atom(conn, "_NET_WM_PID")?;
    let reply = conn
        .get_property(false, window, atom, AtomEnum::CARDINAL, 0, 1)?
        .reply()?;
    Ok(reply.value32().and_then(|mut i| i.next()))
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
    let raw = reply.value;
    let mut parts = raw.split(|&b| b == 0).filter(|s| !s.is_empty());
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
