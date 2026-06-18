//! Native-Wayland backend.
//!
//! Used when running under a Wayland compositor with no X11 (WAYLAND_DISPLAY
//! set, DISPLAY unset). The first slice enumerates toplevels via
//! `zwlr_foreign_toplevel_manager_v1` (wlroots: labwc, sway, …) so
//! `list_windows` works on native Wayland. Capture and input slices land next.

use std::collections::HashMap;

use wayland_client::{
    event_created_child,
    protocol::{wl_output::{self, WlOutput}, wl_pointer::ButtonState, wl_registry, wl_seat::WlSeat},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self as ftl_handle, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{
        self as ftl_manager, ZwlrForeignToplevelManagerV1, EVT_TOPLEVEL_OPCODE,
    },
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

/// Linux evdev BTN_LEFT — the button code the virtual-pointer protocol expects.
const BTN_LEFT: u32 = 0x110;

use crate::x11::WindowInfo;

/// Name of the opt-in env var that unlocks the experimental native-Wayland
/// backend.
pub const ENABLE_WAYLAND_ENV: &str = "CUA_DRIVER_RS_ENABLE_WAYLAND";

/// Whether the user has opted into the experimental native-Wayland backend.
///
/// The Wayland backend is incomplete (toplevel enumeration + virtual-pointer /
/// virtual-keyboard input via the wlroots protocols; capture and AT-SPI parity
/// are still landing), so it stays OFF by default and a pure-Wayland session is
/// reported as unsupported unless the user explicitly sets
/// `CUA_DRIVER_RS_ENABLE_WAYLAND=1`. Any value other than empty / `0` / `false`
/// enables it.
pub fn wayland_enabled() -> bool {
    match std::env::var(ENABLE_WAYLAND_ENV) {
        Ok(v) => {
            let v = v.trim();
            !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
        }
        Err(_) => false,
    }
}

/// True when we should drive Wayland rather than X11: the experimental backend
/// is opted in ([`wayland_enabled`]), a Wayland display is present, and there is
/// no X11 DISPLAY to fall back to. Without the opt-in this returns false even on
/// a pure-Wayland session, so the backend treats it as unsupported rather than
/// silently engaging an incomplete code path.
pub fn is_wayland() -> bool {
    wayland_enabled()
        && std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var_os("DISPLAY").is_none()
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
    // Virtual-pointer manager + output dimensions, so `click` can land a real
    // button press at the output centre (over the just-activated window).
    vptr_manager: Option<ZwlrVirtualPointerManagerV1>,
    output_w: u32,
    output_h: u32,
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
            } else if interface == ZwlrVirtualPointerManagerV1::interface().name {
                state.vptr_manager =
                    Some(registry.bind::<ZwlrVirtualPointerManagerV1, _, _>(name, version.min(2), qh, ()));
            } else if interface == WlOutput::interface().name {
                registry.bind::<WlOutput, _, _>(name, version.min(4), qh, ());
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

impl Dispatch<WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Remember the output resolution so `click` can aim at its centre.
        if let wl_output::Event::Mode { width, height, .. } = event {
            state.output_w = width.max(0) as u32;
            state.output_h = height.max(0) as u32;
        }
    }
}

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerManagerV1,
        _: <ZwlrVirtualPointerManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrVirtualPointerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerV1,
        _: <ZwlrVirtualPointerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
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

/// Click a native Wayland toplevel identified by its `window_id` (the
/// foreign-toplevel protocol id from `list_windows`). Wayland forbids a client
/// from knowing another window's on-screen geometry, so we cannot map the
/// caller's window-local x/y to a global pointer position. Instead, the
/// focused-input model is two steps:
///   1. `activate` the target toplevel (foreign-toplevel) — focus+raise it, so
///      on a stacking layout it fills the output.
///   2. land a real virtual-pointer button press at the output centre (now over
///      the target). This both visually clicks and — crucially on sway, whose
///      headless seat has no keyboard until a pointer interaction occurs — makes
///      the compositor route the subsequent virtual-keyboard input to the
///      target. (labwc routes it from `activate` alone; the centre click is
///      harmless there since the activated window is centred under the cursor.)
pub fn click(window_id: u64) -> anyhow::Result<()> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State::default();
    queue.roundtrip(&mut state)?; // bind manager + seat + vptr + outputs
    if state.manager.is_none() {
        anyhow::bail!("compositor does not expose zwlr_foreign_toplevel_manager_v1");
    }
    for _ in 0..4 {
        queue.roundtrip(&mut state)?; // drain toplevel/handle creation + output mode
    }

    let id = window_id as u32;
    let handle = state
        .handles
        .get(&id)
        .ok_or_else(|| anyhow::anyhow!("no native Wayland toplevel for window_id {window_id}"))?
        .clone();
    let seat = state
        .seat
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor exposed no wl_seat to activate the window"))?;
    handle.activate(&seat);
    queue.roundtrip(&mut state)?; // flush activate so the target is focused/raised

    // Land a button press at the output centre (over the now-focused window).
    if let Some(mgr) = state.vptr_manager.clone() {
        let (w, h) = (state.output_w.max(1), state.output_h.max(1));
        let vptr = mgr.create_virtual_pointer(Some(&seat), &qh, ());
        vptr.motion_absolute(0, w / 2, h / 2, w, h);
        vptr.frame();
        vptr.button(0, BTN_LEFT, ButtonState::Pressed);
        vptr.frame();
        vptr.button(0, BTN_LEFT, ButtonState::Released);
        vptr.frame();
        queue.roundtrip(&mut state)?;
        vptr.destroy();
        queue.roundtrip(&mut state)?;
    }
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
    // Lead with a no-op Shift_L tap: on a freshly-focused window under a headless
    // seat (notably sway), the compositor needs the first virtual-keyboard event
    // to wire up keyboard routing, and that first key is dropped. Sacrificing a
    // modifier tap (no character) absorbs the drop so the real text lands intact;
    // it's harmless where routing is already live (labwc).
    let out = std::process::Command::new("wtype")
        .args(["-k", "Shift_L", "--"])
        .arg(text)
        .output()?;
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

// ── EIS nested-compositor injection ────────────────────────────────────────
//
// When cua-driver's nested compositor is `cua-compositor` (our patched wlroots,
// see nix/cua-driver/compositor/), it exposes a line-protocol control socket at
// $CUA_INJECT_SOCKET for what stock Wayland forbids: focus-FREE per-surface
// keyboard injection and MULTI-cursor pointer injection, both routed to a target
// window by its xdg app_id. These helpers speak that protocol.

/// The control socket path, when running against the EIS nested compositor.
pub fn inject_socket_path() -> Option<String> {
    std::env::var("CUA_INJECT_SOCKET").ok().filter(|s| !s.is_empty())
}

/// True when input should be routed through the EIS compositor's control socket
/// (focus-free / multi-cursor) rather than wtype / virtual-pointer.
pub fn is_inject_mode() -> bool {
    inject_socket_path().is_some()
}

fn inject_send(lines: &[String]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    let path = inject_socket_path().ok_or_else(|| anyhow::anyhow!("CUA_INJECT_SOCKET not set"))?;
    // The nested compositor may still be starting; retry the connect briefly.
    let mut stream = None;
    for _ in 0..60 {
        match UnixStream::connect(&path) {
            Ok(s) => { stream = Some(s); break; }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    let mut s = stream.ok_or_else(|| anyhow::anyhow!("could not connect to inject socket {path}"))?;
    let mut buf = String::new();
    for l in lines {
        buf.push_str(l);
        buf.push('\n');
    }
    s.write_all(buf.as_bytes())?;
    s.flush()?;
    // Give the compositor a moment to process before the socket closes.
    std::thread::sleep(std::time::Duration::from_millis(80));
    Ok(())
}

/// Resolve a window_id (foreign-toplevel protocol id) to its xdg app_id by
/// enumerating toplevels — the inject protocol addresses windows by app_id.
pub fn app_id_for_window(window_id: u64) -> Option<String> {
    let conn = Connection::connect_to_env().ok()?;
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut state = State::default();
    queue.roundtrip(&mut state).ok()?;
    for _ in 0..4 {
        queue.roundtrip(&mut state).ok()?;
    }
    state
        .toplevels
        .get(&(window_id as u32))
        .map(|t| t.app_id.clone())
        .filter(|s| !s.is_empty())
}

fn to_hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

/// Map a cua/X11 mouse-button number to its evdev (wl_pointer) button code.
fn evdev_button(x_button: u32) -> u32 {
    match x_button {
        3 => 0x111, // BTN_RIGHT
        2 => 0x112, // BTN_MIDDLE
        _ => 0x110, // BTN_LEFT
    }
}

/// Focus-free type into the window's surface (no focus change).
pub fn inject_type_text(window_id: u64, text: &str) -> anyhow::Result<()> {
    let app = app_id_for_window(window_id)
        .ok_or_else(|| anyhow::anyhow!("no Wayland app_id for window {window_id}"))?;
    inject_send(&[format!("t {app} {}", to_hex(text))])
}

/// Focus-free named-key press into the window's surface.
pub fn inject_press_key(window_id: u64, key: &str) -> anyhow::Result<()> {
    let app = app_id_for_window(window_id)
        .ok_or_else(|| anyhow::anyhow!("no Wayland app_id for window {window_id}"))?;
    inject_send(&[format!("k {app} {key}")])
}

/// A single pointer drag for `inject_parallel_drags`: window-local waypoints,
/// driven by cursor `idx` so several run concurrently on one window.
pub struct InjectDrag {
    pub app_id: String,
    pub idx: usize,
    pub x_button: u32,
    pub path: Vec<(f64, f64)>,
    pub steps: usize,
}

fn resample(path: &[(f64, f64)], steps: usize) -> Vec<(f64, f64)> {
    if path.len() < 2 || steps == 0 {
        return path.to_vec();
    }
    let seglen: Vec<f64> = path
        .windows(2)
        .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
        .collect();
    let total: f64 = seglen.iter().sum();
    if total == 0.0 {
        return vec![path[0]; steps + 1];
    }
    let mut out = Vec::with_capacity(steps + 1);
    for s in 0..=steps {
        let target = total * (s as f64) / (steps as f64);
        let mut acc = 0.0;
        let mut pt = path[path.len() - 1];
        for (i, &l) in seglen.iter().enumerate() {
            if acc + l >= target || i == seglen.len() - 1 {
                let f = if l > 0.0 { (target - acc) / l } else { 0.0 };
                pt = (path[i].0 + (path[i + 1].0 - path[i].0) * f,
                      path[i].1 + (path[i + 1].1 - path[i].1) * f);
                break;
            }
            acc += l;
        }
        out.push(pt);
    }
    out
}

/// Run N pointer drags concurrently on their target windows: each cursor presses
/// at its start, glides through its (interleaved) waypoints, and releases. This
/// is true multi-cursor — each `idx` is an independent cursor in the compositor.
pub fn inject_parallel_drags(drags: &[InjectDrag]) -> anyhow::Result<()> {
    if drags.is_empty() {
        return Ok(());
    }
    let resampled: Vec<Vec<(f64, f64)>> =
        drags.iter().map(|d| resample(&d.path, d.steps.max(1))).collect();
    let max_steps = resampled.iter().map(|p| p.len()).max().unwrap_or(0);
    let mut lines = Vec::new();
    // Press each cursor at its start point.
    for (d, pts) in drags.iter().zip(&resampled) {
        let (x, y) = pts[0];
        lines.push(format!("m {} {} {x:.1} {y:.1}", d.app_id, d.idx));
        lines.push(format!("b {} {} {} 1", d.app_id, d.idx, evdev_button(d.x_button)));
    }
    // Glide all cursors together, one interleaved step at a time.
    for s in 1..max_steps {
        for (d, pts) in drags.iter().zip(&resampled) {
            let (x, y) = pts[s.min(pts.len() - 1)];
            lines.push(format!("m {} {} {x:.1} {y:.1}", d.app_id, d.idx));
        }
    }
    // Release each cursor.
    for (d, _) in drags.iter().zip(&resampled) {
        lines.push(format!("b {} {} {} 0", d.app_id, d.idx, evdev_button(d.x_button)));
    }
    inject_send(&lines)
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
