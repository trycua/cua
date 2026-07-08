//! Native-Wayland backend.
//!
//! Used when running under a Wayland compositor with no X11 (WAYLAND_DISPLAY
//! set, DISPLAY unset). Enumerates toplevels via
//! `zwlr_foreign_toplevel_manager_v1`, captures per-output screenshots via
//! `zwlr_screencopy_manager_v1` + `wl_shm` (native — `grim` remains a
//! fallback), and synthesises pointer / scroll / drag input via
//! `zwlr_virtual_pointer_v1`. Per-window image capture is deferred until
//! `ext-foreign-toplevel-image-capture-source-v1` lands in
//! `wayland-protocols-wlr`; until then `screenshot_window_dispatch` returns a
//! typed error on pure Wayland.

pub mod ext_screencopy;
pub mod overlay;
pub mod persistent_vptr;
pub mod portal_screenshot;
pub mod shell_helper;
// `portal_screencast` (PipeWire per-window capture) and `libei` (GNOME/KDE
// input via xdg-desktop-portal RemoteDesktop) need libpipewire-0.3 and reis
// at build time, which the cross-platform release container (debian:11,
// GLIBC_2.31 floor) can't satisfy without bumping the floor. They're behind
// the `portal-libei` feature so the published binaries stay portable; the
// Nix build (which already has modern PipeWire + libei from nixpkgs)
// enables it. Wlroots screencopy + virtual-pointer remain unconditional.
#[cfg(feature = "portal-libei")]
pub mod libei;
#[cfg(feature = "portal-libei")]
pub mod portal_screencast;

/// Whether this binary was compiled with the `portal-libei` feature — the
/// xdg-desktop-portal RemoteDesktop + libei input path. It is the ONLY input
/// backend that works on non-wlroots compositors (KWin/Plasma, Mutter/GNOME),
/// which do not implement `zwlr_virtual_pointer_v1`. The published
/// curl-pipe-bash tarball is built WITHOUT it (#1967 — debian:11 CD container
/// lacks a new-enough PipeWire/libei), so on those compositors input injection
/// has no backend and silently no-ops. Consulted by the doctor and the input
/// dispatch so that failure is reported instead of hidden. See #1982.
pub const PORTAL_LIBEI_ENABLED: bool = cfg!(feature = "portal-libei");

use std::collections::HashMap;

use wayland_client::{
    event_created_child,
    protocol::{
        wl_buffer::WlBuffer,
        wl_output::{self, WlOutput},
        wl_pointer::{Axis, AxisSource, ButtonState},
        wl_registry,
        wl_seat::WlSeat,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
    },
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self as ftl_handle, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{
        self as ftl_manager, ZwlrForeignToplevelManagerV1, EVT_TOPLEVEL_OPCODE,
    },
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self as scrcopy_frame, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
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
/// The Wayland backend covers toplevel enumeration, per-output capture
/// (native screencopy + a `grim` fallback), and virtual-pointer /
/// virtual-keyboard input via the wlroots protocols. Per-window image
/// capture still depends on the staging `ext-image-copy-capture-v1`
/// protocol, so the backend stays OFF by default and a pure-Wayland
/// session is reported as unsupported unless the user explicitly sets
/// `CUA_DRIVER_RS_ENABLE_WAYLAND=1`. Any value other than empty / `0` /
/// `false` enables it.
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

/// Reason string when X11 input injection cannot possibly work, so callers
/// **fail loudly** instead of falling through to an X11 path that no-ops yet
/// reports success. Triggers only on a *pure* Wayland session — `WAYLAND_DISPLAY`
/// set, no X11 `DISPLAY` — with the native-Wayland backend NOT opted in (so
/// [`is_wayland`] is false and the X11 path would be chosen). XWayland sessions
/// (where `DISPLAY` is set) and X11 sessions return `None` and proceed normally.
/// See #1921.
pub fn wayland_input_unavailable_reason() -> Option<String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var_os("DISPLAY").is_none()
        && !wayland_enabled()
    {
        Some(format!(
            "input cannot be delivered: pure Wayland session (no X11 DISPLAY) and \
             the native-Wayland input backend is not enabled. Set {}=1 to enable \
             the Wayland backend (wlroots compositors: sway, labwc, hyprland), or \
             run the target under XWayland so an X11 DISPLAY is available.",
            ENABLE_WAYLAND_ENV
        ))
    } else {
        None
    }
}

fn wl_sockets(dir: &str) -> std::collections::HashSet<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| {
            n.strip_prefix("wayland-")
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        })
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
                        tracing::error!(
                            "cua nested compositor '{comp}': no Wayland socket appeared"
                        );
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

/// Per-capture in-flight state populated by the screencopy frame Dispatch.
#[derive(Default)]
struct CaptureState {
    /// wl_shm format code (Argb8888 / Xrgb8888 / …).
    format: Option<u32>,
    width: u32,
    height: u32,
    stride: u32,
    y_invert: bool,
    ready: bool,
    failed: bool,
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
    output: Option<WlOutput>,
    output_w: u32,
    output_h: u32,
    // Native screencopy capture state.
    scrcopy_manager: Option<ZwlrScreencopyManagerV1>,
    shm: Option<WlShm>,
    capture: CaptureState,
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
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == ZwlrForeignToplevelManagerV1::interface().name {
                let v = version.min(3);
                state.manager =
                    Some(registry.bind::<ZwlrForeignToplevelManagerV1, _, _>(name, v, qh, ()));
            } else if interface == WlSeat::interface().name {
                let v = version.min(7);
                state.seat = Some(registry.bind::<WlSeat, _, _>(name, v, qh, ()));
            } else if interface == ZwlrVirtualPointerManagerV1::interface().name {
                state.vptr_manager = Some(registry.bind::<ZwlrVirtualPointerManagerV1, _, _>(
                    name,
                    version.min(2),
                    qh,
                    (),
                ));
            } else if interface == WlOutput::interface().name {
                let out = registry.bind::<WlOutput, _, _>(name, version.min(4), qh, ());
                if state.output.is_none() {
                    state.output = Some(out);
                }
            } else if interface == ZwlrScreencopyManagerV1::interface().name {
                state.scrcopy_manager = Some(registry.bind::<ZwlrScreencopyManagerV1, _, _>(
                    name,
                    version.min(3),
                    qh,
                    (),
                ));
            } else if interface == WlShm::interface().name {
                state.shm = Some(registry.bind::<WlShm, _, _>(name, version.min(1), qh, ()));
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

impl Dispatch<WlShm, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // wl_shm advertises supported formats via `format` events; we don't
        // need to track them — screencopy tells us exactly which format to use
        // for the frame buffer.
    }
}

impl Dispatch<WlShmPool, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: wayland_client::protocol::wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        _: wayland_client::protocol::wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrScreencopyManagerV1,
        _: <ZwlrScreencopyManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: scrcopy_frame::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            scrcopy_frame::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                if let WEnum::Value(fmt) = format {
                    state.capture.format = Some(fmt as u32);
                }
                state.capture.width = width;
                state.capture.height = height;
                state.capture.stride = stride;
            }
            scrcopy_frame::Event::Flags { flags } => {
                if let WEnum::Value(f) = flags {
                    state.capture.y_invert = f.contains(scrcopy_frame::Flags::YInvert);
                }
            }
            scrcopy_frame::Event::Ready { .. } => {
                state.capture.ready = true;
            }
            scrcopy_frame::Event::Failed => {
                state.capture.failed = true;
            }
            _ => {}
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
        out.push(WindowInfo {
            xid: *id as u64,
            pid: None,
            title,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
    }
    Ok(out)
}

// ── Capture (native screencopy + grim fallback) ──────────────────────────────

/// Capture the Wayland output as PNG bytes via `zwlr_screencopy_manager_v1`.
///
/// Binds the screencopy manager plus `wl_shm`, asks the compositor to copy the
/// next frame of the first advertised output into a shm buffer, channel-swaps
/// from the compositor's pixel format to RGBA, and encodes a PNG via the
/// existing `image` crate. Falls back to shelling out to `grim` when the
/// screencopy manager or `wl_shm` is unavailable so users on lighter wlroots
/// builds stay supported.
pub fn screenshot_bytes() -> anyhow::Result<Vec<u8>> {
    match capture_via_screencopy() {
        Ok(bytes) => return Ok(bytes),
        Err(e) => tracing::warn!("native screencopy failed, falling back to grim: {e}"),
    }
    capture_via_grim()
}

/// Shell out to `grim -t png -` — the wlroots reference screenshot tool. Kept
/// as the last-resort fallback for compositors that hide screencopy.
fn capture_via_grim() -> anyhow::Result<Vec<u8>> {
    let out = std::process::Command::new("grim")
        .args(["-t", "png", "-"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("grim failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    if out.stdout.is_empty() {
        anyhow::bail!("grim produced no output");
    }
    Ok(out.stdout)
}

/// Native screencopy path: bind manager + shm, allocate an anon mmap buffer,
/// request a copy, wait for Ready, swap channels, encode PNG. Returns an error
/// if any global is missing or the compositor flags the capture as failed.
fn capture_via_screencopy() -> anyhow::Result<Vec<u8>> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State::default();
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?; // outputs report their Mode

    let manager = state
        .scrcopy_manager
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor does not expose zwlr_screencopy_manager_v1"))?;
    let shm = state
        .shm
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor does not expose wl_shm"))?;
    let output = state
        .output
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor exposed no wl_output to capture"))?;

    let frame = manager.capture_output(0, &output, &qh, ());
    // Drain Buffer / Flags events; spin until Ready or Failed (or timeout).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut buffer: Option<WlBuffer> = None;
    let mut pool: Option<WlShmPool> = None;
    let mut mmap_ptr: *mut libc::c_void = std::ptr::null_mut();
    let mut mmap_len: usize = 0;
    let mut fd: i32 = -1;

    loop {
        queue.roundtrip(&mut state)?;
        if state.capture.failed {
            anyhow::bail!("compositor signalled screencopy failure");
        }
        if state.capture.ready {
            break;
        }
        // Once we know the buffer params, allocate + send copy exactly once.
        if buffer.is_none()
            && state.capture.format.is_some()
            && state.capture.stride > 0
            && state.capture.height > 0
        {
            let size = (state.capture.stride as usize)
                .checked_mul(state.capture.height as usize)
                .ok_or_else(|| anyhow::anyhow!("screencopy buffer size overflow"))?;
            let (anon_fd, p) = anon_shm(size)?;
            fd = anon_fd;
            mmap_ptr = p;
            mmap_len = size;
            use std::os::fd::AsFd as _;
            let pool_fd = unsafe { borrowed_fd(fd) };
            let p = shm.create_pool(pool_fd.as_fd(), size as i32, &qh, ());
            let fmt_raw = state.capture.format.unwrap();
            let fmt: wl_shm::Format = match wl_shm::Format::try_from(fmt_raw) {
                Ok(f) => f,
                Err(_) => {
                    cleanup_mmap(mmap_ptr, mmap_len, fd);
                    anyhow::bail!("compositor advertised unsupported wl_shm format {fmt_raw:#x}");
                }
            };
            let b = p.create_buffer(
                0,
                state.capture.width as i32,
                state.capture.height as i32,
                state.capture.stride as i32,
                fmt,
                &qh,
                (),
            );
            frame.copy(&b);
            buffer = Some(b);
            pool = Some(p);
        }
        if std::time::Instant::now() >= deadline {
            cleanup_mmap(mmap_ptr, mmap_len, fd);
            anyhow::bail!("screencopy timed out waiting for frame");
        }
    }

    let result = (|| -> anyhow::Result<Vec<u8>> {
        let w = state.capture.width;
        let h = state.capture.height;
        let stride = state.capture.stride as usize;
        let format = state.capture.format.unwrap_or(0);
        if mmap_ptr.is_null() || mmap_len == 0 {
            anyhow::bail!("screencopy ready without a backing buffer");
        }
        let raw = unsafe { std::slice::from_raw_parts(mmap_ptr as *const u8, mmap_len) };
        let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for row in 0..(h as usize) {
            let src_row = if state.capture.y_invert {
                (h as usize) - 1 - row
            } else {
                row
            };
            let base = src_row * stride;
            for col in 0..(w as usize) {
                let px = &raw[base + col * 4..base + col * 4 + 4];
                let (r, g, b, a) = match wl_shm::Format::try_from(format).ok() {
                    // Argb8888 / Xrgb8888 over wl_shm are little-endian BGRA / BGRX.
                    Some(wl_shm::Format::Argb8888) => (px[2], px[1], px[0], px[3]),
                    Some(wl_shm::Format::Xrgb8888) => (px[2], px[1], px[0], 255),
                    Some(wl_shm::Format::Abgr8888) => (px[0], px[1], px[2], px[3]),
                    Some(wl_shm::Format::Xbgr8888) => (px[0], px[1], px[2], 255),
                    _ => (px[2], px[1], px[0], px[3]),
                };
                rgba.extend_from_slice(&[r, g, b, a]);
            }
        }
        cua_driver_core::image_utils::encode_rgba_to_png(&rgba, w, h)
    })();

    // Always tear down regardless of result.
    if let Some(b) = buffer {
        b.destroy();
    }
    if let Some(p) = pool {
        p.destroy();
    }
    frame.destroy();
    let _ = queue.roundtrip(&mut state);
    cleanup_mmap(mmap_ptr, mmap_len, fd);

    result
}

/// Allocate an anonymous shared-memory file of `size` bytes and mmap it RW.
/// Returns the raw fd and the mmap pointer; the caller is responsible for
/// passing both to [`cleanup_mmap`] when done.
pub(crate) fn anon_shm(size: usize) -> anyhow::Result<(i32, *mut libc::c_void)> {
    // memfd_create is Linux-only and is the cleanest path; fall back to
    // shm_open if memfd isn't available for any reason.
    let name = b"cua-scrcopy\0";
    let fd = unsafe { libc::memfd_create(name.as_ptr() as *const libc::c_char, libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(anyhow::anyhow!(
            "memfd_create failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let rc = unsafe { libc::ftruncate(fd, size as libc::off_t) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(anyhow::anyhow!("ftruncate failed: {err}"));
    }
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    if p == libc::MAP_FAILED {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(anyhow::anyhow!("mmap failed: {err}"));
    }
    Ok((fd, p))
}

/// Unmap and close the screencopy backing buffer; safe to call with the
/// sentinel values left from a never-allocated buffer.
pub(crate) fn cleanup_mmap(ptr: *mut libc::c_void, len: usize, fd: i32) {
    if !ptr.is_null() && len > 0 {
        unsafe { libc::munmap(ptr, len) };
    }
    if fd >= 0 {
        unsafe { libc::close(fd) };
    }
}

/// Borrow a raw fd as an `OwnedFd` for wl_shm.create_pool. The pool keeps
/// its own reference; we close our copy via [`cleanup_mmap`].
///
/// SAFETY: caller must guarantee `fd` is a valid open file descriptor.
/// `libc::dup` may fail (returning -1, errno set), in which case we panic
/// instead of constructing an `OwnedFd` from -1 (which would have UB on
/// drop). Callers that need fallible behaviour should use the dup syscall
/// directly and check the result before wrapping.
pub(crate) unsafe fn borrowed_fd(fd: i32) -> std::os::fd::OwnedFd {
    use std::os::fd::FromRawFd;
    let dup = libc::dup(fd);
    if dup < 0 {
        let err = std::io::Error::last_os_error();
        panic!("dup({fd}) failed: {err}");
    }
    std::os::fd::OwnedFd::from_raw_fd(dup)
}

/// Capture dispatcher: native Wayland (screencopy with grim fallback) when
/// applicable, else X11. Mirrors `screenshot_window_dispatch` for the
/// output-level path used by `get_window_state`'s vision payload.
pub fn screenshot_dispatch(xid: u64) -> anyhow::Result<Vec<u8>> {
    if is_wayland() {
        screenshot_bytes()
    } else {
        crate::capture::screenshot_window_bytes(xid)
    }
}

/// Display-level capture dispatcher. Cascade:
/// 1. Native Wayland on wlroots: zwlr_screencopy_manager_v1 (fast, zero
///    consent).
/// 2. Wayland but no wlroots screencopy globals (GNOME/KDE/COSMIC):
///    xdg-desktop-portal Screenshot via ashpd. Triggers consent prompt
///    on first use per session.
/// 3. X11: existing root-window path.
pub fn screenshot_display_dispatch() -> anyhow::Result<Vec<u8>> {
    if is_wayland() {
        // Tier 1: native wlroots screencopy (fast, zero consent).
        match screenshot_bytes() {
            Ok(bytes) => return Ok(bytes),
            Err(e) => {
                tracing::debug!(
                    "wlroots screencopy unavailable ({e}); trying ext-image-copy-capture-v1"
                );
            }
        }
        // Tier 2: ext-image-copy-capture-v1 (sway 1.10+, labwc 0.8+, niri,
        // hyprland, KDE 6.2+, GNOME 47+).
        match ext_screencopy::screenshot_via_ext_copy() {
            Ok(bytes) => return Ok(bytes),
            Err(e) => {
                tracing::debug!(
                    "ext-image-copy-capture-v1 unavailable ({e}); trying xdg-desktop-portal"
                );
            }
        }
        // Tier 3: xdg-desktop-portal (GNOME, KDE, COSMIC fallback).
        match portal_screenshot::screenshot_via_portal() {
            Ok(bytes) => return Ok(bytes),
            Err(e) => {
                tracing::debug!(
                    "xdg-desktop-portal Screenshot unavailable ({e}); falling through to X11"
                );
            }
        }
    }
    // Final fallback: X11 root window. Call the X11-only path explicitly
    // so we don't re-enter screenshot_display_bytes (which routes back here
    // on Wayland — would loop forever).
    crate::capture::screenshot_display_bytes_x11()
}

/// Per-window capture dispatcher. On X11 forwards to the existing window
/// capture path; on pure Wayland returns a typed error pointing at the
/// staging `ext-image-copy-capture-v1` protocol — wlr-screencopy is
/// output-only, and `foreign-toplevel` exposes no per-window geometry to
/// crop with.
pub fn screenshot_window_dispatch(xid: u64) -> anyhow::Result<Vec<u8>> {
    if is_wayland() {
        anyhow::bail!(
            "per-window screenshot is not yet supported on native Wayland — \
             zwlr_screencopy_manager_v1 is output-only and ext-image-copy-capture-v1 \
             is not yet shipped in wayland-protocols-wlr. Run under XWayland to crop \
             to a single window, or capture the full output instead."
        );
    }
    crate::capture::screenshot_window_bytes(xid)
}

// ── Input session helper ─────────────────────────────────────────────────────

/// Live virtual-pointer session: connection + queue + the bound objects every
/// pointer op (click, scroll, drag) needs. Returned by [`open_vptr_session`].
pub struct VptrSession {
    pub conn: Connection,
    queue: wayland_client::EventQueue<State>,
    state: State,
    pub seat: WlSeat,
    pub vptr: ZwlrVirtualPointerV1,
    pub output_w: u32,
    pub output_h: u32,
}

/// Bind manager + seat + virtual-pointer + first output, optionally activate a
/// foreign-toplevel by `window_id` so the synthesised events land on it, and
/// return the live session that scroll / drag / click reuse. Wayland forbids a
/// client from knowing another window's on-screen geometry, so we drive every
/// pointer event in *output* coordinates and rely on the activated toplevel
/// covering the centre.
pub fn open_vptr_session(activate_window_id: Option<u32>) -> anyhow::Result<VptrSession> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State::default();
    queue.roundtrip(&mut state)?;
    if state.manager.is_none() {
        anyhow::bail!("compositor does not expose zwlr_foreign_toplevel_manager_v1");
    }
    for _ in 0..4 {
        queue.roundtrip(&mut state)?;
    }

    let seat = state.seat.clone().ok_or_else(|| {
        anyhow::anyhow!("compositor exposed no wl_seat for virtual-pointer input")
    })?;
    let mgr = state.vptr_manager.clone().ok_or_else(|| {
        if PORTAL_LIBEI_ENABLED {
            anyhow::anyhow!("compositor does not expose zwlr_virtual_pointer_manager_v1")
        } else {
            // KWin/Plasma and Mutter/GNOME don't implement zwlr_virtual_pointer,
            // and this build has no libei/portal fallback — so input has no
            // backend at all rather than silently no-op'ing. See #1982.
            anyhow::anyhow!(
                "no input backend for this compositor: it exposes no \
                 zwlr_virtual_pointer_manager_v1 and this build was compiled \
                 without libei/portal support (#1982). Use the portal-enabled \
                 Linux build for input on KDE Plasma / GNOME, or a wlroots \
                 compositor (sway, labwc, hyprland)."
            )
        }
    })?;

    if let Some(id) = activate_window_id {
        let handle = state
            .handles
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no native Wayland toplevel for window_id {id}"))?;
        handle.activate(&seat);
        queue.roundtrip(&mut state)?;
    }

    let vptr = mgr.create_virtual_pointer(Some(&seat), &qh, ());
    let (output_w, output_h) = (state.output_w.max(1), state.output_h.max(1));
    Ok(VptrSession {
        conn,
        queue,
        state,
        seat,
        vptr,
        output_w,
        output_h,
    })
}

/// Map a cua/X11 pointer button (1=left / 2=middle / 3=right) to its evdev
/// code, which is what `zwlr_virtual_pointer_v1::button` expects.
pub fn evdev_pointer_button(button: u8) -> u32 {
    match button {
        2 => 0x112, // BTN_MIDDLE
        3 => 0x111, // BTN_RIGHT
        _ => 0x110, // BTN_LEFT
    }
}

/// Click a native Wayland toplevel identified by its `window_id` (the
/// foreign-toplevel protocol id from `list_windows`) at output-relative
/// `(x, y)`, with `button` (1/2/3 = left/middle/right) emitted `count` times.
/// Coordinates default to the output centre when both x and y are zero so
/// the legacy focus-based behaviour is preserved when callers can't supply
/// real coords. A short delay between iterations gives the compositor time
/// to discriminate single vs. double clicks.
pub fn click(window_id: u64, x: i32, y: i32, count: u32, button: u8) -> anyhow::Result<()> {
    let mut sess = open_vptr_session(Some(window_id as u32))?;
    let (w, h) = (sess.output_w, sess.output_h);
    let (px, py) = if x == 0 && y == 0 {
        ((w / 2) as i32, (h / 2) as i32)
    } else {
        (x, y)
    };
    let px = px.clamp(0, w as i32 - 1) as u32;
    let py = py.clamp(0, h as i32 - 1) as u32;
    let btn = evdev_pointer_button(button);
    for i in 0..count.max(1) {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
        sess.vptr.motion_absolute(0, px, py, w, h);
        sess.vptr.frame();
        sess.vptr.button(0, btn, ButtonState::Pressed);
        sess.vptr.frame();
        sess.vptr.button(0, btn, ButtonState::Released);
        sess.vptr.frame();
        sess.queue.roundtrip(&mut sess.state)?;
    }
    // Keep the synthetic-cursor registry in sync with the warp we just
    // performed so a subsequent `get_cursor_position` reflects reality.
    record_synth_cursor(px as i32, py as i32);
    sess.vptr.destroy();
    sess.queue.roundtrip(&mut sess.state)?;
    Ok(())
}

/// Synthesize a vertical or horizontal scroll on the activated toplevel. Each
/// tick emits an `axis_source(wheel)` + `axis_discrete(1)` pair through the
/// virtual-pointer protocol, mirroring how a real wheel notch decomposes. The
/// magnitude follows wl_pointer convention: ±10 (in wl_fixed = ×256) per tick.
pub fn scroll(window_id: u64, direction: &str, amount: u32) -> anyhow::Result<()> {
    let mut sess = open_vptr_session(Some(window_id as u32))?;
    let (axis, sign): (Axis, i32) = match direction.to_ascii_lowercase().as_str() {
        "up" => (Axis::VerticalScroll, -1),
        "down" => (Axis::VerticalScroll, 1),
        "left" => (Axis::HorizontalScroll, -1),
        "right" => (Axis::HorizontalScroll, 1),
        other => anyhow::bail!("unknown scroll direction: {other}"),
    };
    // axis_discrete: `value` is logical units (the wayland-rs wrapper
    // converts to wl_fixed internally); `discrete` is the tick count.
    let value: f64 = (sign as f64) * 10.0;
    for i in 0..amount.max(1) {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        sess.vptr.axis_source(AxisSource::Wheel);
        sess.vptr.axis_discrete(0, axis, value, sign);
        sess.vptr.frame();
        sess.queue.roundtrip(&mut sess.state)?;
    }
    sess.vptr.destroy();
    sess.queue.roundtrip(&mut sess.state)?;
    Ok(())
}

/// Last cursor position the agent warped to via `move_cursor_absolute`.
/// Wayland exposes no protocol for clients to read the real global cursor
/// position; a Wayland-conformant `get_cursor_position` can therefore only
/// report what THIS process synthesized. Updated every `motion_absolute`
/// emitted from `move_cursor_absolute` / `click` / `drag`.
static SYNTH_CURSOR_POS: std::sync::OnceLock<std::sync::Mutex<Option<(i32, i32)>>> =
    std::sync::OnceLock::new();

fn record_synth_cursor(x: i32, y: i32) {
    let cell = SYNTH_CURSOR_POS.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(mut g) = cell.lock() {
        *g = Some((x, y));
    }
}

/// Returns the last `(x, y)` this process warped the cursor to via the
/// Wayland virtual-pointer protocol, or `None` if no warp has happened in
/// this process. The reading is "synthetic": Wayland forbids clients from
/// querying the real cursor position, so this value diverges from reality
/// the moment the user moves their physical mouse. Callers should surface
/// `source: "synthetic"` in the structured payload.
pub fn last_synth_cursor_pos() -> Option<(i32, i32)> {
    SYNTH_CURSOR_POS
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .ok()
        .and_then(|g| *g)
}

/// Warp the cursor to absolute output coordinates `(x, y)` using
/// `zwlr_virtual_pointer_v1::motion_absolute`. Clamps to the output bounds
/// reported by `open_vptr_session`. Emits a motion + frame and roundtrips so
/// the compositor commits the warp before returning. Records the position in
/// the synthetic-cursor registry so `last_synth_cursor_pos` can report it.
pub fn move_cursor_absolute(window_id: Option<u64>, x: i32, y: i32) -> anyhow::Result<()> {
    let mut sess = open_vptr_session(window_id.map(|w| w as u32))?;
    let (w, h) = (sess.output_w, sess.output_h);
    let px = x.clamp(0, (w as i32).saturating_sub(1)) as u32;
    let py = y.clamp(0, (h as i32).saturating_sub(1)) as u32;
    sess.vptr.motion_absolute(0, px, py, w, h);
    sess.vptr.frame();
    sess.queue.roundtrip(&mut sess.state)?;
    record_synth_cursor(px as i32, py as i32);
    sess.vptr.destroy();
    sess.queue.roundtrip(&mut sess.state)?;
    Ok(())
}

/// Press-drag-release on a native Wayland toplevel. Emits one button press at
/// `(from_x, from_y)`, then `steps` interpolated motion events along the
/// straight segment to `(to_x, to_y)`, then a release. Coordinates are
/// output-relative; window-local coords need the EIS inject socket
/// (`CUA_INJECT_SOCKET`).
pub fn drag(
    window_id: u64,
    from_x: i32,
    from_y: i32,
    to_x: i32,
    to_y: i32,
    steps: u32,
    button: u8,
) -> anyhow::Result<()> {
    let mut sess = open_vptr_session(Some(window_id as u32))?;
    let (w, h) = (sess.output_w, sess.output_h);
    let btn = evdev_pointer_button(button);
    let clamp_xy = |x: i32, y: i32| -> (u32, u32) {
        (
            x.clamp(0, w as i32 - 1) as u32,
            y.clamp(0, h as i32 - 1) as u32,
        )
    };
    let (fx, fy) = clamp_xy(from_x, from_y);
    sess.vptr.motion_absolute(0, fx, fy, w, h);
    sess.vptr.frame();
    sess.vptr.button(0, btn, ButtonState::Pressed);
    sess.vptr.frame();
    sess.queue.roundtrip(&mut sess.state)?;
    let n = steps.max(1);
    for s in 1..=n {
        let t = s as f64 / n as f64;
        let ix = (from_x as f64 + (to_x - from_x) as f64 * t).round() as i32;
        let iy = (from_y as f64 + (to_y - from_y) as f64 * t).round() as i32;
        let (cx, cy) = clamp_xy(ix, iy);
        sess.vptr.motion_absolute(0, cx, cy, w, h);
        sess.vptr.frame();
        sess.queue.roundtrip(&mut sess.state)?;
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    let (tx, ty) = clamp_xy(to_x, to_y);
    sess.vptr.motion_absolute(0, tx, ty, w, h);
    sess.vptr.frame();
    sess.vptr.button(0, btn, ButtonState::Released);
    sess.vptr.frame();
    // Sync the synthetic-cursor registry with the drag endpoint so a
    // subsequent `get_cursor_position` reports where we left the pointer.
    record_synth_cursor(tx as i32, ty as i32);
    sess.queue.roundtrip(&mut sess.state)?;
    sess.vptr.destroy();
    sess.queue.roundtrip(&mut sess.state)?;
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
    let out = std::process::Command::new("wtype")
        .args(["-k", &keysym])
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "wtype -k {keysym} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Press a key combination (modifiers + final key) via `wtype`. Each modifier
/// is pressed before the key, then released after, exactly like
/// `wtype -M ctrl -M shift -k key -m shift -m ctrl`. Unknown values pass
/// straight to wtype's `-k` so single-character keys and X keysym names work
/// as-is. This is the Wayland equivalent of the X11 `send_key` modifier mask.
pub fn hotkey(keys: &[String]) -> anyhow::Result<()> {
    let (mods, final_key) = partition_modifiers(keys)?;
    let keysym = key_to_keysym(&final_key);
    let mut args: Vec<String> = Vec::new();
    for m in &mods {
        args.push("-M".into());
        args.push(m.clone());
    }
    args.push("-k".into());
    args.push(keysym.clone());
    // Release modifiers in reverse press order.
    for m in mods.iter().rev() {
        args.push("-m".into());
        args.push(m.clone());
    }
    let out = std::process::Command::new("wtype").args(&args).output()?;
    if !out.status.success() {
        anyhow::bail!(
            "wtype {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Split a `keys` array into wtype-compatible modifier names and a single
/// final key. Recognised modifier inputs: ctrl/control, alt, shift,
/// super/meta/cmd/command/win/windows. The final key must be the one
/// non-modifier in the list.
fn partition_modifiers(keys: &[String]) -> anyhow::Result<(Vec<String>, String)> {
    let mut mods: Vec<String> = Vec::new();
    let mut non_mods: Vec<String> = Vec::new();
    for k in keys {
        match k.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.push("ctrl".into()),
            "alt" => mods.push("alt".into()),
            "shift" => mods.push("shift".into()),
            "super" | "meta" | "cmd" | "command" | "win" | "windows" => mods.push("logo".into()),
            _ => non_mods.push(k.clone()),
        }
    }
    let final_key = non_mods
        .last()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("hotkey requires at least one non-modifier key"))?;
    Ok((mods, final_key))
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
    std::env::var("CUA_INJECT_SOCKET")
        .ok()
        .filter(|s| !s.is_empty())
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
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    let mut s =
        stream.ok_or_else(|| anyhow::anyhow!("could not connect to inject socket {path}"))?;
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

/// Resolve the WM_CLASS-equivalent (instance, class) pair for a window. On
/// X11 reads `WM_CLASS`; on Wayland reuses [`app_id_for_window`] and returns
/// `(app_id, app_id)` — the closest analogue, since foreign-toplevel exposes
/// a single app_id and not the X11 instance/class split. Used by terminal
/// emulator detection on `is_terminal_window` so Ghostty / kitty / alacritty
/// are recognised on Wayland too.
pub fn wm_class_dispatch(window_id: u64) -> Option<(String, String)> {
    if is_wayland() {
        let app = app_id_for_window(window_id)?;
        return Some((app.clone(), app));
    }
    crate::x11::wm_class_for_window(window_id)
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

/// Focus-free click into the window's surface via the nested EIS compositor.
/// Coordinates are window-local, matching the rest of the inject protocol.
pub fn inject_click(window_id: u64, x: f64, y: f64, count: u32, button: u8) -> anyhow::Result<()> {
    let app = app_id_for_window(window_id)
        .ok_or_else(|| anyhow::anyhow!("no Wayland app_id for window {window_id}"))?;
    let btn = evdev_button(button as u32);
    let n = count.max(1);
    let mut lines = Vec::with_capacity((n as usize) * 4);
    for i in 0..n {
        if i > 0 {
            // The line protocol is batch-oriented, so use a tiny move-only
            // separator between clicks to give the compositor a frame boundary
            // without introducing a protocol-level sleep primitive.
            lines.push(format!("m {app} 0 {x:.1} {y:.1}"));
        }
        lines.push(format!("m {app} 0 {x:.1} {y:.1}"));
        lines.push(format!("b {app} 0 {btn} 1"));
        lines.push(format!("b {app} 0 {btn} 0"));
    }
    inject_send(&lines)
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
                pt = (
                    path[i].0 + (path[i + 1].0 - path[i].0) * f,
                    path[i].1 + (path[i + 1].1 - path[i].1) * f,
                );
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
    let resampled: Vec<Vec<(f64, f64)>> = drags
        .iter()
        .map(|d| resample(&d.path, d.steps.max(1)))
        .collect();
    let max_steps = resampled.iter().map(|p| p.len()).max().unwrap_or(0);
    let mut lines = Vec::new();
    // Press each cursor at its start point.
    for (d, pts) in drags.iter().zip(&resampled) {
        let (x, y) = pts[0];
        lines.push(format!("m {} {} {x:.1} {y:.1}", d.app_id, d.idx));
        lines.push(format!(
            "b {} {} {} 1",
            d.app_id,
            d.idx,
            evdev_button(d.x_button)
        ));
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
        lines.push(format!(
            "b {} {} {} 0",
            d.app_id,
            d.idx,
            evdev_button(d.x_button)
        ));
    }
    inject_send(&lines)
}

/// Window-enumeration dispatcher: native Wayland when applicable, else X11.
pub fn list_windows_dispatch(filter_pid: Option<u32>) -> Vec<WindowInfo> {
    if is_wayland() {
        // wlroots compositors expose zwlr_foreign_toplevel_management — use it
        // (it has no pid, so filter_pid can't apply there).
        match list_windows() {
            Ok(ws) if !ws.is_empty() => return ws,
            Ok(_) => {
                // GNOME Mutter / KDE KWin don't implement foreign-toplevel, so the
                // list came back empty. Native Wayland apps have no X11 XID either,
                // so fall back to enumerating windows from the AT-SPI registry
                // (keyed by pid — the same tree get_window_state walks).
                let ws = crate::atspi::list_windows(filter_pid);
                if !ws.is_empty() {
                    return ws;
                }
            }
            Err(e) => {
                tracing::warn!(
                    "wayland foreign-toplevel list_windows failed: {e}; trying AT-SPI registry"
                );
                let ws = crate::atspi::list_windows(filter_pid);
                if !ws.is_empty() {
                    return ws;
                }
            }
        }
        // Last resort under Wayland: an Xwayland app may still have an X11 XID.
    }
    // On an XWayland-co-present Wayland session (GNOME/Mutter, KDE/KWin: DISPLAY
    // is set, so `is_wayland()` above is false), X11 enumeration only sees
    // XWayland apps — native Wayland apps have no X11 XID and are otherwise
    // invisible (#1978). Merge in the AT-SPI registry (keyed by pid), which does
    // surface native Wayland apps that expose accessibility. Gated on the same
    // opt-in as the rest of the native-Wayland backend.
    //
    // Caveats for the merged AT-SPI entries: they carry a synthetic (non-X11)
    // xid and zero geometry (x/y/w/h = 0), like the existing wlroots AT-SPI
    // fallback — so `bring_to_front` / `screenshot_window` / pixel translation
    // against them error cleanly rather than acting (input on GNOME/KDE routes
    // by pid + screen coords, not xid, so it's unaffected). Dedup is per-pid, so
    // the rare app owning BOTH an XWayland window and a separate native-Wayland
    // toplevel would list only the XWayland one.
    let mut ws = crate::x11::list_windows(filter_pid);
    if wayland_enabled() && std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let seen: std::collections::HashSet<u32> = ws.iter().filter_map(|w| w.pid).collect();
        // A specific pid already resolved via X11 needs no AT-SPI walk (a full
        // D-Bus enumeration of every registered app): it can only add duplicates.
        let already_covered = filter_pid.map_or(false, |p| seen.contains(&p));
        if !already_covered {
            for w in crate::atspi::list_windows(filter_pid) {
                // XWayland apps appear in both lists; keep the X11 entry (real
                // XID + geometry) and add only AT-SPI windows for pids X11 didn't
                // report.
                if w.pid.map_or(true, |p| !seen.contains(&p)) {
                    ws.push(w);
                }
            }
        }
    }
    ws
}

/// Snapshot of which wlroots manager globals the running compositor advertises.
/// Used by the `health_report` Wayland backend probe to distinguish a working
/// session from one missing screencopy or virtual-pointer support.
#[derive(Default, Clone, Debug)]
pub struct WaylandManagers {
    pub foreign_toplevel: bool,
    pub screencopy: bool,
    pub virtual_pointer: bool,
    pub wl_shm: bool,
    /// Staging `ext-image-copy-capture-v1` manager — sway 1.10+, labwc
    /// 0.8+, niri, KDE 6.2+, GNOME mutter 47+.
    pub ext_image_copy_capture: bool,
    /// Companion `ext-output-image-capture-source-v1` source manager —
    /// required to capture a `wl_output` via the staging protocol.
    pub ext_output_image_capture_source: bool,
}

/// Perform a single registry roundtrip and report which of the manager
/// interfaces the doctor cares about advertise themselves. Returns `Err` only
/// when we can't even open a Wayland connection — a successful connect with
/// no managers still resolves to an all-false snapshot.
pub fn probe_managers() -> anyhow::Result<WaylandManagers> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut state = State::default();
    queue.roundtrip(&mut state)?;
    // Reuse the existing State for wlroots managers, then do a parallel
    // probe for staging ext-image-copy-capture interfaces by walking the
    // raw registry events (no binding required — we only need presence).
    let ext_probe = probe_ext_interfaces().unwrap_or_default();
    Ok(WaylandManagers {
        foreign_toplevel: state.manager.is_some(),
        screencopy: state.scrcopy_manager.is_some(),
        virtual_pointer: state.vptr_manager.is_some(),
        wl_shm: state.shm.is_some(),
        ext_image_copy_capture: ext_probe.image_copy_capture,
        ext_output_image_capture_source: ext_probe.output_image_capture_source,
    })
}

#[derive(Default, Clone, Copy, Debug)]
struct ExtInterfaceProbe {
    image_copy_capture: bool,
    output_image_capture_source: bool,
}

/// Probe registry for `ext-image-copy-capture-v1` + companion source manager
/// presence without binding them. Cheap (one roundtrip) and side-effect
/// free.
fn probe_ext_interfaces() -> anyhow::Result<ExtInterfaceProbe> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<ExtProbeState>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut state = ExtProbeState::default();
    queue.roundtrip(&mut state)?;
    Ok(state.probe)
}

#[derive(Default)]
struct ExtProbeState {
    probe: ExtInterfaceProbe,
}

impl Dispatch<wl_registry::WlRegistry, ()> for ExtProbeState {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { interface, .. } = event {
            match interface.as_str() {
                "ext_image_copy_capture_manager_v1" => {
                    state.probe.image_copy_capture = true;
                }
                "ext_output_image_capture_source_manager_v1" => {
                    state.probe.output_image_capture_source = true;
                }
                _ => {}
            }
        }
    }
}

// Suppress dead-code warning for the unused BTN_LEFT alias kept for backward
// compatibility with earlier slice constants.
#[allow(dead_code)]
const _BTN_LEFT_ALIAS: u32 = BTN_LEFT;
