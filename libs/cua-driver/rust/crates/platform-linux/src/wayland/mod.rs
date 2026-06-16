//! Native-Wayland backend.
//!
//! Used when running under a Wayland compositor with no X11 (WAYLAND_DISPLAY
//! set, DISPLAY unset). The first slice enumerates toplevels via
//! `zwlr_foreign_toplevel_manager_v1` (wlroots: labwc, sway, …) so
//! `list_windows` works on native Wayland. Capture and input slices land next.

use std::collections::HashMap;

use wayland_client::{
    event_created_child, protocol::wl_registry, Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self as ftl_handle, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{
        self as ftl_manager, ZwlrForeignToplevelManagerV1, EVT_TOPLEVEL_OPCODE,
    },
};

use crate::x11::WindowInfo;

/// True when we should drive Wayland rather than X11: a Wayland display is
/// present and there is no X11 DISPLAY to fall back to.
pub fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_none()
}

#[derive(Default)]
struct Toplevel {
    title: String,
    app_id: String,
    closed: bool,
}

#[derive(Default)]
struct State {
    manager: Option<ZwlrForeignToplevelManagerV1>,
    toplevels: HashMap<u32, Toplevel>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            if interface == ZwlrForeignToplevelManagerV1::interface().name {
                let v = version.min(3);
                state.manager = Some(registry.bind::<ZwlrForeignToplevelManagerV1, _, _>(name, v, qh, ()));
            }
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        _state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        _event: ftl_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The `toplevel` event creates a handle object (see event_created_child!);
        // the handle's own events carry the title/app_id we collect below.
    }

    event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: ftl_handle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = handle.id().protocol_id();
        let tl = state.toplevels.entry(id).or_default();
        match event {
            ftl_handle::Event::Title { title } => tl.title = title,
            ftl_handle::Event::AppId { app_id } => tl.app_id = app_id,
            ftl_handle::Event::Closed => tl.closed = true,
            _ => {}
        }
    }
}

/// Enumerate native Wayland toplevels via wlr-foreign-toplevel-management.
/// `xid` is the foreign-toplevel handle's protocol id (a stable per-session
/// window id); pid is unknown (not exposed by the protocol); geometry is 0
/// (the protocol does not surface position/size). app_id is folded into the
/// title (`"<title> [<app_id>]"`) so callers matching on either still match.
pub fn list_windows() -> anyhow::Result<Vec<WindowInfo>> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State::default();
    queue.roundtrip(&mut state)?; // registry globals -> bind manager
    if state.manager.is_none() {
        anyhow::bail!("compositor does not expose zwlr_foreign_toplevel_manager_v1");
    }
    // Manager emits a `toplevel` per window; each handle then emits title/app_id
    // and a `done`. A few roundtrips drain the initial enumeration.
    for _ in 0..4 {
        queue.roundtrip(&mut state)?;
    }

    let mut out = Vec::new();
    for (id, tl) in &state.toplevels {
        if tl.closed {
            continue;
        }
        let title = if tl.app_id.is_empty() {
            tl.title.clone()
        } else {
            format!("{} [{}]", tl.title, tl.app_id)
        };
        out.push(WindowInfo { xid: *id as u64, pid: None, title, x: 0, y: 0, width: 0, height: 0 });
    }
    Ok(out)
}

/// Capture the Wayland output as PNG bytes via `grim` (the wlroots screenshot
/// tool — wlr-screencopy under the hood, works on labwc/sway). This mirrors the
/// X11 backend shelling out to `import`/`xwd`. foreign-toplevel exposes no
/// per-window geometry, so this captures the whole output; that is sufficient
/// for `get_window_state`'s vision payload.
pub fn screenshot_bytes() -> anyhow::Result<Vec<u8>> {
    let out = std::process::Command::new("grim").args(["-t", "png", "-"]).output()?;
    if !out.status.success() {
        anyhow::bail!("grim failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    if out.stdout.is_empty() {
        anyhow::bail!("grim produced no output");
    }
    Ok(out.stdout)
}

/// Capture dispatcher: native Wayland (grim) when applicable, else X11.
pub fn screenshot_dispatch(xid: u64) -> anyhow::Result<Vec<u8>> {
    if is_wayland() {
        screenshot_bytes()
    } else {
        crate::capture::screenshot_window_bytes(xid)
    }
}

/// Window-enumeration dispatcher: native Wayland when applicable, else X11.
pub fn list_windows_dispatch(filter_pid: Option<u32>) -> Vec<WindowInfo> {
    if is_wayland() {
        match list_windows() {
            Ok(ws) => return ws, // foreign-toplevel has no pid, so filter_pid can't apply
            Err(e) => tracing::warn!("wayland list_windows failed, falling back to X11: {e}"),
        }
    }
    crate::x11::list_windows(filter_pid)
}
