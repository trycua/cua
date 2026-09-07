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
    pub app_name: String,
    pub title: String,
    pub is_on_screen: bool,
    pub z_index: Option<usize>,
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
    window_owner_matches(get_window_pid(&conn, xid).ok().flatten(), pid)
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

    let mut result = Vec::new();
    for (z_index, xid) in windows.into_iter().enumerate() {
        let pid = get_window_pid(&conn, xid).ok().flatten();
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
            z_index: Some(z_index_from_bottom_to_top(z_index)),
            x,
            y,
            width: w,
            height: h,
        });
    }

    Ok(result)
}

fn z_index_from_bottom_to_top(position: usize) -> usize {
    position
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
                if ewmh_client_list_is_usable(reply.type_, &windows) {
                    return Ok(windows);
                }
            }
        }
    }

    // Rootless XWayland WMs can omit the EWMH client lists, or publish them
    // while leaving them empty, then reparent each real client beneath a
    // compositor-owned frame. The same fallback covers bare X. Walk only
    // bounded, viewable descendants and admit ICCCM client windows with an
    // exact PID, title, and WM_STATE. This excludes compositor frames, support
    // windows, and our overlay while avoiding unmapped Electron startup rows.
    nested_client_windows(conn, root)
}

fn ewmh_client_list_is_usable(property_type: Atom, windows: &[Window]) -> bool {
    property_type != x11rb::NONE && !windows.is_empty()
}

const MAX_FALLBACK_TREE_DEPTH: usize = 8;
const MAX_FALLBACK_TREE_WINDOWS: usize = 4096;

fn nested_client_windows(conn: &RustConnection, root: Window) -> Result<Vec<Window>> {
    let root_children = conn.query_tree(root)?.reply()?.children;
    if !fallback_tree_within_budget(0, 0, root_children.len()) {
        anyhow::bail!(
            "X11 fallback tree exceeds the {MAX_FALLBACK_TREE_WINDOWS}-window discovery limit"
        );
    }
    let wm_state = get_atom(conn, "WM_STATE")?;
    let mut pending = root_children
        .into_iter()
        .rev()
        .map(|window| (window, 1_usize))
        .collect::<Vec<_>>();
    let mut visited = 0_usize;
    let mut clients = Vec::new();

    while let Some((window, depth)) = pending.pop() {
        visited += 1;
        if !window_is_viewable(conn, window) {
            continue;
        }
        if fallback_window_is_client(conn, window, wm_state) {
            clients.push(window);
            continue;
        }
        if depth >= MAX_FALLBACK_TREE_DEPTH {
            continue;
        }
        if let Ok(tree) = conn.query_tree(window)?.reply() {
            if !fallback_tree_within_budget(visited, pending.len(), tree.children.len()) {
                anyhow::bail!(
                    "X11 fallback tree exceeds the {MAX_FALLBACK_TREE_WINDOWS}-window discovery limit"
                );
            }
            pending.extend(
                tree.children
                    .into_iter()
                    .rev()
                    .map(|child| (child, depth + 1)),
            );
        }
    }

    Ok(clients)
}

fn fallback_tree_within_budget(visited: usize, pending: usize, additional: usize) -> bool {
    visited
        .checked_add(pending)
        .and_then(|known| known.checked_add(additional))
        .is_some_and(|known| known <= MAX_FALLBACK_TREE_WINDOWS)
}

fn window_is_viewable(conn: &RustConnection, window: Window) -> bool {
    conn.get_window_attributes(window)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_some_and(|attributes| fallback_window_is_listable(attributes.map_state))
}

fn fallback_window_is_client(conn: &RustConnection, window: Window, wm_state: Atom) -> bool {
    let has_pid = get_window_pid(conn, window).ok().flatten().is_some();
    let has_title = get_window_title(conn, window).is_ok_and(|title| !title.trim().is_empty());
    let has_wm_state = conn
        .get_property(false, window, wm_state, AtomEnum::ANY, 0, 2)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_some_and(|reply| reply.type_ != x11rb::NONE);
    fallback_client_is_eligible(has_pid, has_title, has_wm_state)
}

fn fallback_client_is_eligible(has_pid: bool, has_title: bool, has_wm_state: bool) -> bool {
    has_pid && has_title && has_wm_state
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
    let owner = get_window_pid(&conn, xid)?;
    match owner {
        Some(owner) if owner == pid => {}
        Some(owner) => anyhow::bail!("window_id {xid} belongs to pid {owner}, not pid {pid}"),
        None => anyhow::bail!("window_id {xid} has no verifiable _NET_WM_PID owner"),
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
    get_window_class(&conn, xid as u32)
}

fn get_window_class(conn: &RustConnection, xid: Window) -> Option<(String, String)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a live X11 server"]
    fn live_nested_client_fallback_lists_reparented_client() {
        use x11rb::protocol::xproto::ConnectionExt as _;
        use x11rb::wrapper::ConnectionExt as _;

        let (conn, screen_num) = RustConnection::connect(None).expect("connect to X11 fixture");
        let screen = &conn.setup().roots[screen_num];
        let frame = conn.generate_id().expect("generate frame window id");
        let client = conn.generate_id().expect("generate client window id");

        conn.create_window(
            screen.root_depth,
            frame,
            screen.root,
            20,
            20,
            360,
            240,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().background_pixel(screen.black_pixel),
        )
        .expect("create frame request")
        .check()
        .expect("create frame");
        conn.create_window(
            screen.root_depth,
            client,
            frame,
            4,
            4,
            320,
            200,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().background_pixel(screen.white_pixel),
        )
        .expect("create client request")
        .check()
        .expect("create client");

        let pid_atom = get_atom(&conn, "_NET_WM_PID").expect("intern pid atom");
        let name_atom = get_atom(&conn, "_NET_WM_NAME").expect("intern name atom");
        let utf8_atom = get_atom(&conn, "UTF8_STRING").expect("intern UTF8 atom");
        let wm_state = get_atom(&conn, "WM_STATE").expect("intern WM_STATE atom");
        conn.change_property32(
            PropMode::REPLACE,
            client,
            pid_atom,
            AtomEnum::CARDINAL,
            &[std::process::id()],
        )
        .expect("set client pid");
        conn.change_property8(
            PropMode::REPLACE,
            client,
            name_atom,
            utf8_atom,
            b"nested-client-fixture",
        )
        .expect("set client title");
        conn.change_property32(
            PropMode::REPLACE,
            client,
            wm_state,
            wm_state,
            &[1, x11rb::NONE],
        )
        .expect("set client WM_STATE");

        let stacking = get_atom(&conn, "_NET_CLIENT_LIST_STACKING").expect("intern stacking atom");
        let client_list = get_atom(&conn, "_NET_CLIENT_LIST").expect("intern client-list atom");
        conn.change_property32(
            PropMode::REPLACE,
            screen.root,
            stacking,
            AtomEnum::WINDOW,
            &[],
        )
        .expect("publish empty stacking list");
        conn.delete_property(screen.root, client_list)
            .expect("remove client list");
        conn.map_window(client).expect("map client");
        conn.map_window(frame).expect("map frame");
        conn.flush().expect("flush fixture");
        conn.get_input_focus()
            .expect("fixture sync request")
            .reply()
            .expect("fixture sync reply");

        let windows = list_windows(Some(std::process::id()));
        assert_eq!(windows.len(), 1, "only the nested ICCCM client is listable");
        assert_eq!(windows[0].xid, u64::from(client));
        assert_eq!(windows[0].title, "nested-client-fixture");
        assert!(windows[0].is_on_screen);
    }

    #[test]
    fn nested_client_fallback_is_strictly_bounded() {
        assert_eq!(MAX_FALLBACK_TREE_DEPTH, 8);
        assert_eq!(MAX_FALLBACK_TREE_WINDOWS, 4096);
        assert!(fallback_tree_within_budget(1, 4094, 1));
        assert!(!fallback_tree_within_budget(1, 4094, 2));
        assert!(!fallback_tree_within_budget(usize::MAX, 0, 1));
    }

    #[test]
    fn empty_or_absent_ewmh_client_lists_require_fallback_discovery() {
        assert!(!ewmh_client_list_is_usable(1, &[]));
        assert!(!ewmh_client_list_is_usable(x11rb::NONE, &[0x400004]));
        assert!(ewmh_client_list_is_usable(1, &[0x400004]));
    }

    #[test]
    fn nested_client_fallback_requires_exact_client_metadata() {
        assert!(fallback_client_is_eligible(true, true, true));
        assert!(!fallback_client_is_eligible(false, true, true));
        assert!(!fallback_client_is_eligible(true, false, true));
        assert!(!fallback_client_is_eligible(true, true, false));
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
    fn ewmh_bottom_to_top_order_normalizes_to_higher_is_frontmost() {
        let indices: Vec<_> = (0..3).map(z_index_from_bottom_to_top).collect();
        assert_eq!(indices, vec![0, 1, 2]);
        assert!(indices[2] > indices[0]);
    }

    #[test]
    fn moveresize_requests_static_gravity_and_all_frame_fields() {
        let flags = moveresize_window_flags();
        assert_eq!(flags & 0xff, 10);
        assert_eq!(flags & 0x0f00, 0x0f00);
        assert_eq!(flags & !0x0fff, 0);
    }
}
