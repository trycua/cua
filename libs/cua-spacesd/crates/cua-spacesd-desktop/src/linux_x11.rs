// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! X11 desktop facts and window management for the Linux desktop backend.
//!
//! Everything here is plain EWMH/ICCCM over x11rb: window enumeration with
//! kinds, states and stacking, phantom-window filtering, displays through
//! RandR monitors, the pointer position, and window management requests
//! (activate, minimize, maximize, restore, close, move/resize) sent to the
//! window manager rather than applied behind its back.

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

use cua_spacesd_provider_api::{ProviderDisplay, ProviderError, ProviderErrorCode};

/// Broad role of a top-level window (mirrors `cua.env.v1.WindowKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum X11WindowKind {
    Standard,
    Dialog,
    Panel,
    Menu,
    Tooltip,
    System,
    Phantom,
}

/// Window state (mirrors `cua.env.v1.WindowState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum X11WindowState {
    Normal,
    Minimized,
    Maximized,
    Fullscreen,
    Hidden,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct X11Window {
    pub xid: u32,
    pub pid: Option<u32>,
    pub title: String,
    /// WM_CLASS class (falls back to the instance).
    pub app_name: String,
    /// WM_CLASS instance, usable as a desktop-file hint.
    pub app_id: String,
    /// Frame of the client window in root coordinates.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub mapped: bool,
    pub state: X11WindowState,
    pub kind: X11WindowKind,
    pub focused: bool,
    /// 0 is frontmost among listed windows.
    pub z_order: u32,
}

pub(crate) fn x_error(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(ProviderErrorCode::Internal, format!("X11: {error}"))
}

pub(crate) fn connect() -> Result<(RustConnection, Window), ProviderError> {
    let (conn, screen) = RustConnection::connect(None).map_err(|error| {
        ProviderError::new(
            ProviderErrorCode::TargetUnavailable,
            format!(
                "cannot connect to the X server (DISPLAY={:?}): {error}",
                std::env::var("DISPLAY").ok()
            ),
        )
    })?;
    let root = conn.setup().roots[screen].root;
    Ok((conn, root))
}

pub(crate) fn atom(conn: &RustConnection, name: &str) -> Option<Atom> {
    conn.intern_atom(false, name.as_bytes())
        .ok()?
        .reply()
        .ok()
        .map(|reply| reply.atom)
}

fn atoms32(conn: &RustConnection, window: Window, name: &str, kind: impl Into<Atom>) -> Vec<u32> {
    let Some(property) = atom(conn, name) else {
        return Vec::new();
    };
    conn.get_property(false, window, property, kind.into(), 0, 4096)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| reply.value32().map(|values| values.collect()))
        .unwrap_or_default()
}

pub(crate) fn window_title(conn: &RustConnection, window: Window) -> String {
    if let (Some(name), Some(utf8)) = (atom(conn, "_NET_WM_NAME"), atom(conn, "UTF8_STRING")) {
        if let Some(reply) = conn
            .get_property(false, window, name, utf8, 0, 1024)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            if !reply.value.is_empty() {
                return String::from_utf8_lossy(&reply.value).into_owned();
            }
        }
    }
    conn.get_property(false, window, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 1024)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| String::from_utf8_lossy(&reply.value).into_owned())
        .unwrap_or_default()
}

fn window_class(conn: &RustConnection, window: Window) -> (String, String) {
    let value = conn
        .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 1024)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| reply.value)
        .unwrap_or_default();
    let mut parts = value
        .split(|byte| *byte == 0)
        .map(|part| String::from_utf8_lossy(part).into_owned());
    let instance = parts.next().unwrap_or_default();
    let class = parts.next().unwrap_or_default();
    (instance, class)
}

/// Whether an EWMH window manager runs: the root's
/// `_NET_SUPPORTING_WM_CHECK` names a live window that names itself.
pub(crate) fn window_manager_running(conn: &RustConnection, root: Window) -> bool {
    let Some(check) = atoms32(conn, root, "_NET_SUPPORTING_WM_CHECK", AtomEnum::WINDOW)
        .first()
        .copied()
        .filter(|w| *w != x11rb::NONE)
    else {
        return false;
    };
    atoms32(conn, check, "_NET_SUPPORTING_WM_CHECK", AtomEnum::WINDOW).first() == Some(&check)
}

/// Whether the X server offers XTEST, which cua-driver's foreground input
/// needs.
pub(crate) fn has_xtest(conn: &RustConnection) -> bool {
    conn.query_extension(b"XTEST")
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_some_and(|reply| reply.present)
}

pub(crate) fn window_pid(conn: &RustConnection, window: Window) -> Option<u32> {
    atoms32(conn, window, "_NET_WM_PID", AtomEnum::CARDINAL)
        .first()
        .copied()
}

pub(crate) fn active_window(conn: &RustConnection, root: Window) -> Option<Window> {
    atoms32(conn, root, "_NET_ACTIVE_WINDOW", AtomEnum::WINDOW)
        .first()
        .copied()
        .filter(|window| *window != x11rb::NONE)
        .or_else(|| {
            conn.get_input_focus()
                .ok()?
                .reply()
                .ok()
                .map(|focus| focus.focus)
                .filter(|window| *window > 1)
        })
}

fn stacking(conn: &RustConnection, root: Window) -> Vec<Window> {
    for name in ["_NET_CLIENT_LIST_STACKING", "_NET_CLIENT_LIST"] {
        let Some(property) = atom(conn, name) else {
            continue;
        };
        if let Some(reply) = conn
            .get_property(false, root, property, AtomEnum::WINDOW, 0, u32::MAX)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            if reply.type_ != x11rb::NONE {
                return reply
                    .value32()
                    .map(|values| values.collect())
                    .unwrap_or_default();
            }
        }
    }
    // No window manager: mapped, managed-looking root children, bottom to top.
    conn.query_tree(root)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|tree| tree.children)
        .unwrap_or_default()
}

fn window_kind(
    conn: &RustConnection,
    window: Window,
    attributes: &GetWindowAttributesReply,
) -> X11WindowKind {
    if attributes.class == WindowClass::INPUT_ONLY {
        return X11WindowKind::Phantom;
    }
    let types = atoms32(conn, window, "_NET_WM_WINDOW_TYPE", AtomEnum::ATOM);
    for kind in types {
        let name = conn
            .get_atom_name(kind)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| String::from_utf8_lossy(&reply.name).into_owned())
            .unwrap_or_default();
        match name.trim_start_matches("_NET_WM_WINDOW_TYPE_") {
            "NORMAL" => return X11WindowKind::Standard,
            "DIALOG" => return X11WindowKind::Dialog,
            "UTILITY" | "TOOLBAR" | "SPLASH" => return X11WindowKind::Panel,
            "MENU" | "DROPDOWN_MENU" | "POPUP_MENU" | "COMBO" => return X11WindowKind::Menu,
            "TOOLTIP" | "NOTIFICATION" => return X11WindowKind::Tooltip,
            "DESKTOP" | "DOCK" => return X11WindowKind::System,
            "DND" => return X11WindowKind::Phantom,
            _ => {}
        }
    }
    if attributes.override_redirect {
        X11WindowKind::Menu
    } else {
        X11WindowKind::Standard
    }
}

fn window_state(conn: &RustConnection, window: Window, mapped: bool) -> X11WindowState {
    let states = atoms32(conn, window, "_NET_WM_STATE", AtomEnum::ATOM);
    let has = |name: &str| atom(conn, name).is_some_and(|atom| states.contains(&atom));
    if has("_NET_WM_STATE_HIDDEN") {
        X11WindowState::Minimized
    } else if has("_NET_WM_STATE_FULLSCREEN") {
        X11WindowState::Fullscreen
    } else if has("_NET_WM_STATE_MAXIMIZED_VERT") && has("_NET_WM_STATE_MAXIMIZED_HORZ") {
        X11WindowState::Maximized
    } else if !mapped {
        // ICCCM WM_STATE Iconic (3) means minimized; otherwise withdrawn.
        let iconic = atom(conn, "WM_STATE").is_some_and(|wm_state| {
            conn.get_property(false, window, wm_state, wm_state, 0, 2)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .and_then(|reply| reply.value32().and_then(|mut values| values.next()))
                == Some(3)
        });
        if iconic {
            X11WindowState::Minimized
        } else {
            X11WindowState::Hidden
        }
    } else {
        X11WindowState::Normal
    }
}

/// A window no user would consider an application window: input-only,
/// degenerate size, a drag-and-drop proxy, or one of this process's own
/// overlay surfaces (the agent-cursor overlay).
pub(crate) fn is_phantom(window: &X11Window, own_pid: u32) -> bool {
    window.kind == X11WindowKind::Phantom
        || window.width <= 2
        || window.height <= 2
        || window.pid == Some(own_pid)
}

/// Enumerate top-level windows front to back. Phantom windows are marked
/// with `X11WindowKind::Phantom` (callers filter unless asked not to).
pub(crate) fn list_windows(conn: &RustConnection, root: Window) -> Vec<X11Window> {
    let own_pid = std::process::id();
    let focused = active_window(conn, root);
    let mut windows: Vec<X11Window> = Vec::new();
    for xid in stacking(conn, root).into_iter().rev() {
        let Some(attributes) = conn
            .get_window_attributes(xid)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            continue;
        };
        let Some(geometry) = conn
            .get_geometry(xid)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            continue;
        };
        let (x, y) = conn
            .translate_coordinates(xid, root, 0, 0)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| (i32::from(reply.dst_x), i32::from(reply.dst_y)))
            .unwrap_or((i32::from(geometry.x), i32::from(geometry.y)));
        let mapped = attributes.map_state == MapState::VIEWABLE;
        let (instance, class) = window_class(conn, xid);
        let mut window = X11Window {
            xid,
            pid: window_pid(conn, xid),
            title: window_title(conn, xid),
            app_name: if class.is_empty() {
                instance.clone()
            } else {
                class
            },
            app_id: instance,
            x,
            y,
            width: u32::from(geometry.width),
            height: u32::from(geometry.height),
            mapped,
            state: window_state(conn, xid, mapped),
            kind: window_kind(conn, xid, &attributes),
            focused: focused == Some(xid),
            z_order: windows.len() as u32,
        };
        if is_phantom(&window, own_pid) {
            window.kind = X11WindowKind::Phantom;
        }
        windows.push(window);
    }
    windows
}

/// Displays from RandR monitors; falls back to the root window as one
/// display. Xvfb exposes a single monitor.
pub(crate) fn displays(conn: &RustConnection, root: Window) -> Vec<ProviderDisplay> {
    let mut displays = Vec::new();
    if let Some(monitors) = conn
        .randr_get_monitors(root, true)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
    {
        for (index, monitor) in monitors.monitors.iter().enumerate() {
            let name = conn
                .get_atom_name(monitor.name)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|reply| String::from_utf8_lossy(&reply.name).into_owned())
                .unwrap_or_else(|| format!("monitor-{index}"));
            displays.push(ProviderDisplay {
                id: index.to_string(),
                name,
                primary: monitor.primary,
                bounds: (
                    f64::from(monitor.x),
                    f64::from(monitor.y),
                    f64::from(monitor.width),
                    f64::from(monitor.height),
                ),
                native_width_px: u32::from(monitor.width),
                native_height_px: u32::from(monitor.height),
                scale_factor: 1.0,
                refresh_rate_hz: 0,
            });
        }
    }
    if displays.is_empty() {
        if let Some(geometry) = conn
            .get_geometry(root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            displays.push(ProviderDisplay {
                id: "0".into(),
                name: "screen-0".into(),
                primary: true,
                bounds: (
                    0.0,
                    0.0,
                    f64::from(geometry.width),
                    f64::from(geometry.height),
                ),
                native_width_px: u32::from(geometry.width),
                native_height_px: u32::from(geometry.height),
                scale_factor: 1.0,
                refresh_rate_hz: 0,
            });
        }
    }
    if !displays.iter().any(|display| display.primary) {
        if let Some(first) = displays.first_mut() {
            first.primary = true;
        }
    }
    displays
}

pub(crate) fn pointer_position(
    conn: &RustConnection,
    root: Window,
) -> Result<(i32, i32), ProviderError> {
    let reply = conn
        .query_pointer(root)
        .map_err(x_error)?
        .reply()
        .map_err(x_error)?;
    Ok((i32::from(reply.root_x), i32::from(reply.root_y)))
}

fn client_message(
    conn: &RustConnection,
    root: Window,
    window: Window,
    name: &str,
    data: [u32; 5],
) -> Result<(), ProviderError> {
    let message_type = atom(conn, name).ok_or_else(|| x_error(format!("no atom {name}")))?;
    let event = ClientMessageEvent::new(32, window, message_type, data);
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )
    .map_err(x_error)?;
    sync(conn)
}

fn wm_state(
    conn: &RustConnection,
    root: Window,
    window: Window,
    action: u32,
    first: &str,
    second: Option<&str>,
) -> Result<(), ProviderError> {
    let first = atom(conn, first).unwrap_or(0);
    let second = second.and_then(|name| atom(conn, name)).unwrap_or(0);
    client_message(
        conn,
        root,
        window,
        "_NET_WM_STATE",
        [action, first, second, 1, 0],
    )
}

pub(crate) fn activate(
    conn: &RustConnection,
    root: Window,
    window: Window,
) -> Result<(), ProviderError> {
    let _ = conn.map_window(window);
    client_message(
        conn,
        root,
        window,
        "_NET_ACTIVE_WINDOW",
        [1, x11rb::CURRENT_TIME, 0, 0, 0],
    )?;
    let _ = conn.configure_window(
        window,
        &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
    );
    let _ = conn.set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME);
    sync(conn)
}

pub(crate) fn minimize(
    conn: &RustConnection,
    root: Window,
    window: Window,
) -> Result<(), ProviderError> {
    // ICCCM iconify request: WM_CHANGE_STATE with IconicState (3).
    client_message(conn, root, window, "WM_CHANGE_STATE", [3, 0, 0, 0, 0])
}

pub(crate) fn maximize(
    conn: &RustConnection,
    root: Window,
    window: Window,
) -> Result<(), ProviderError> {
    wm_state(
        conn,
        root,
        window,
        1,
        "_NET_WM_STATE_MAXIMIZED_VERT",
        Some("_NET_WM_STATE_MAXIMIZED_HORZ"),
    )
}

pub(crate) fn restore(
    conn: &RustConnection,
    root: Window,
    window: Window,
) -> Result<(), ProviderError> {
    wm_state(
        conn,
        root,
        window,
        0,
        "_NET_WM_STATE_MAXIMIZED_VERT",
        Some("_NET_WM_STATE_MAXIMIZED_HORZ"),
    )?;
    wm_state(conn, root, window, 0, "_NET_WM_STATE_FULLSCREEN", None)?;
    conn.map_window(window).map_err(x_error)?;
    sync(conn)
}

pub(crate) fn close(
    conn: &RustConnection,
    root: Window,
    window: Window,
    force: bool,
) -> Result<(), ProviderError> {
    if force {
        conn.kill_client(window).map_err(x_error)?;
        return sync(conn);
    }
    client_message(
        conn,
        root,
        window,
        "_NET_CLOSE_WINDOW",
        [x11rb::CURRENT_TIME, 1, 0, 0, 0],
    )
}

pub(crate) fn set_bounds(
    conn: &RustConnection,
    _root: Window,
    window: Window,
    position: Option<(i32, i32)>,
    size: Option<(u32, u32)>,
) -> Result<(), ProviderError> {
    // A ConfigureWindow on the client becomes a ConfigureRequest that the
    // window manager applies (ICCCM 4.1.5); without a window manager it
    // applies directly. Positions are the client's root position.
    let mut aux = ConfigureWindowAux::new();
    if let Some((x, y)) = position {
        // Keep the client (not the frame) at the requested point: offset by
        // the decoration the WM added above/left of the client.
        let (frame_dx, frame_dy) = conn
            .translate_coordinates(window, _root, 0, 0)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .zip(
                conn.get_geometry(window)
                    .ok()
                    .and_then(|cookie| cookie.reply().ok()),
            )
            .map(|(absolute, geometry)| {
                // `geometry` is relative to the parent (the WM frame).
                let _ = absolute;
                (i32::from(geometry.x), i32::from(geometry.y))
            })
            .unwrap_or((0, 0));
        aux = aux.x(x - frame_dx).y(y - frame_dy);
    }
    if let Some((width, height)) = size {
        aux = aux.width(width.max(1)).height(height.max(1));
    }
    conn.configure_window(window, &aux)
        .map_err(x_error)?
        .check()
        .map_err(x_error)?;
    sync(conn)
}

/// Round-trip to the server so every request sent on this short-lived
/// connection is processed before the connection is dropped (a bare flush
/// is not enough: requests can be lost with the connection).
pub(crate) fn sync(conn: &RustConnection) -> Result<(), ProviderError> {
    conn.get_input_focus()
        .map_err(x_error)?
        .reply()
        .map_err(x_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> X11Window {
        X11Window {
            xid: 5,
            pid: Some(1),
            title: "t".into(),
            app_name: "a".into(),
            app_id: "a".into(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            mapped: true,
            state: X11WindowState::Normal,
            kind: X11WindowKind::Standard,
            focused: false,
            z_order: 0,
        }
    }

    #[test]
    fn phantom_filter_rejects_degenerate_input_only_and_own_windows() {
        assert!(!is_phantom(&window(), 99));
        assert!(is_phantom(
            &X11Window {
                width: 1,
                ..window()
            },
            99
        ));
        assert!(is_phantom(
            &X11Window {
                kind: X11WindowKind::Phantom,
                ..window()
            },
            99
        ));
        assert!(is_phantom(
            &X11Window {
                pid: Some(99),
                ..window()
            },
            99
        ));
    }
}
