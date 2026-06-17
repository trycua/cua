//! Native-Wayland backend.
//!
//! Used when running under a Wayland compositor with no X11 (WAYLAND_DISPLAY
//! set, DISPLAY unset). The first slice enumerates toplevels via
//! `zwlr_foreign_toplevel_manager_v1` (wlroots: labwc, sway, …) so
//! `list_windows` works on native Wayland. Capture and input slices land next.

use std::collections::HashMap;

use wayland_client::{
    event_created_child,
    protocol::{wl_registry, wl_seat::WlSeat},
    Connection, Dispatch, Proxy, QueueHandle,
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

fn wl_sockets(dir: &str) -> std::collections::HashSet<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.strip_prefix("wayland-").is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())))
        .collect()
}

/// "Bring your own compositor": if `CUA_WAYLAND_NEST` is set, spawn a private
/// **headless wlroots compositor** (labwc by default) and point this process —
/// and therefore every app it launches (`launch_app`), every capture (`grim`),
/// and all enumeration/injection — at it via `WAYLAND_DISPLAY`. This lets
/// cua-driver automate apps in its **own** Wayland session on ANY host,
/// including KDE (kwin) and GNOME (mutter) which expose no client protocols for
/// this, without ever touching the host compositor or its focus. Idempotent.
pub fn ensure_nested_session() {
    use std::sync::OnceLock;
    static DONE: OnceLock<()> = OnceLock::new();
    if std::env::var_os("CUA_WAYLAND_NEST").is_none() {
        return;
    }
    DONE.get_or_init(|| {
        let xdg = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/0".into());
        let comp = std::env::var("CUA_WAYLAND_NEST_COMPOSITOR").unwrap_or_else(|_| "labwc".into());
        let before = wl_sockets(&xdg);
        let spawned = std::process::Command::new(&comp)
            .env("WLR_BACKENDS", "headless")
            .env("WLR_RENDERER", "pixman")
            .env("WLR_RENDERER_ALLOW_SOFTWARE", "1")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .env("WLR_HEADLESS_OUTPUTS", "1")
            .env_remove("WAYLAND_DISPLAY") // headless: do not nest into the host compositor
            .env_remove("DISPLAY")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match spawned {
            Ok(child) => {
                std::mem::forget(child); // keep the compositor alive for our lifetime
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                loop {
                    if let Some(sock) = wl_sockets(&xdg).difference(&before).min().cloned() {
                        std::env::set_var("WAYLAND_DISPLAY", &sock);
                        std::env::remove_var("DISPLAY");
                        // Publish the nested socket so external tools (e.g. a
                        // `grim` recorder) can target the same session we drive.
                        let _ = std::fs::write(format!("{xdg}/.cua-nested-display"), &sock);
                        tracing::info!("cua nested compositor '{comp}' up: WAYLAND_DISPLAY={sock}");
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        tracing::error!("cua nested compositor '{comp}': no Wayland socket appeared");
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
            Err(e) => tracing::error!("cua nested compositor '{comp}' spawn failed: {e}"),
        }
    });
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
    // Live handles + a seat, kept so `click` can `activate` a target toplevel by
    // its window_id (foreign-toplevel protocol id) — the focus-based input model.
    handles: HashMap<u32, ZwlrForeignToplevelHandleV1>,
    seat: Option<WlSeat>,
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
            } else if interface == WlSeat::interface().name {
                let v = version.min(7);
                state.seat = Some(registry.bind::<WlSeat, _, _>(name, v, qh, ()));
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: wayland_client::protocol::wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Seat name/capabilities events are irrelevant here — we only need the
        // seat object to pass to foreign-toplevel `activate`.
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
        state.handles.entry(id).or_insert_with(|| handle.clone());
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

/// Focus a native Wayland toplevel by its `window_id` (the foreign-toplevel
/// protocol id from `list_windows`) via the protocol's `activate` request — the
/// "click to focus" half of the focused-input model. Wayland forbids a client
/// from knowing another window's on-screen geometry, so we cannot translate the
/// caller's window-local x/y into global pointer coordinates; instead we
/// deterministically focus+raise the exact window the caller targeted, so the
/// subsequent `type_text`/`press_key` (virtual-keyboard) land in it.
pub fn click(window_id: u64) -> anyhow::Result<()> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State::default();
    queue.roundtrip(&mut state)?; // bind manager + seat
    if state.manager.is_none() {
        anyhow::bail!("compositor does not expose zwlr_foreign_toplevel_manager_v1");
    }
    for _ in 0..4 {
        queue.roundtrip(&mut state)?; // drain toplevel + handle creation
    }

    let id = window_id as u32;
    let handle = state
        .handles
        .get(&id)
        .ok_or_else(|| anyhow::anyhow!("no native Wayland toplevel for window_id {window_id}"))?;
    let seat = state
        .seat
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("compositor exposed no wl_seat to activate the window"))?;
    handle.activate(seat);
    queue.roundtrip(&mut state)?; // flush the activate request
    Ok(())
}

/// Type Unicode text into the focused Wayland surface via `wtype` (the
/// virtual-keyboard tool — `zwp_virtual_keyboard_v1` under the hood; it builds
/// the xkb keymap and resolves shift levels for us). This mirrors the X11
/// backend's XSendEvent typing and the capture slice's shell-out to `grim`.
/// foreign-toplevel exposes no pid and Wayland delivers keys to the *focused*
/// surface, so this is window_id-free; pair it with `click`/`activate` to put
/// the intended window in focus first.
pub fn type_text(text: &str) -> anyhow::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let out = std::process::Command::new("wtype").arg("--").arg(text).output()?;
    if !out.status.success() {
        anyhow::bail!("wtype failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// Press a single named key into the focused Wayland surface via `wtype -k`.
pub fn press_key(key: &str) -> anyhow::Result<()> {
    let keysym = key_to_keysym(key);
    let out = std::process::Command::new("wtype").args(["-k", &keysym]).output()?;
    if !out.status.success() {
        anyhow::bail!("wtype -k {keysym} failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// Map cua key names to X keysym names that `wtype -k` understands. Unknown
/// values pass through (single characters and valid keysym names work as-is).
fn key_to_keysym(key: &str) -> String {
    match key.to_lowercase().as_str() {
        "enter" | "return" => "Return",
        "tab" => "Tab",
        "esc" | "escape" => "Escape",
        "space" => "space",
        "backspace" => "BackSpace",
        "delete" | "del" => "Delete",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" | "page_up" => "Prior",
        "pagedown" | "page_down" => "Next",
        _ => return key.to_string(),
    }
    .to_string()
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
