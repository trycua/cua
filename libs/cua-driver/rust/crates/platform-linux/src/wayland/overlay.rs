//! Native-Wayland agent-cursor overlay via `zwlr_layer_shell_v1`.
//!
//! v1 of the layer-shell overlay path. Replaces the X11-only `overlay.rs`
//! render loop on wlroots compositors (sway, labwc, kwin 5.27+, hyprland)
//! by creating a full-screen, click-through, always-on-top `wl_surface`
//! anchored to the first output via `zwlr_layer_shell_v1`. The surface
//! renders a small tinted dot at the current cursor position so the user
//! can see where the agent is acting.
//!
//! Rich gradient-arrow rendering (matching the X11 path) is a follow-up;
//! the surface lifecycle, configure handshake, click-through input region,
//! and command dispatch are all in place here. GNOME mutter does not
//! expose `zwlr_layer_shell_v1` — those sessions either fall through to
//! the X11 path (XWayland) or the nested-compositor mode that spawns
//! labwc internally.
//!
//! Architecture mirrors the existing `wayland/persistent_vptr.rs`: one
//! owner thread (`cua-overlay-wl`) holds the wayland Connection +
//! EventQueue + layer surface; commands flow in over a `crossbeam-channel`.

use std::sync::OnceLock;
use std::thread;

use crossbeam_channel::{bounded, Receiver, Sender};
use cursor_overlay::{OverlayCommand, OverlayMsg, CursorKey};
use wayland_client::{
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_output::WlOutput,
        wl_region::WlRegion,
        wl_registry,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

/// Commands the overlay owner thread accepts. These are a strict subset of
/// the full [`OverlayCommand`] surface — v1 wires position updates and
/// enabled toggling; richer visuals (gradient, click pulse, focus rect)
/// are deferred to a follow-up slice and silently dropped here.
enum WlOverlayCmd {
    Position { key: CursorKey, x: f64, y: f64 },
    SetEnabled { key: CursorKey, enabled: bool },
    Remove { key: CursorKey },
    Shutdown,
}

static TX: OnceLock<Sender<WlOverlayCmd>> = OnceLock::new();

fn tx() -> Option<&'static Sender<WlOverlayCmd>> {
    TX.get()
}

/// Lazily start the owner thread. Idempotent — safe to call from every
/// MCP tool invocation; subsequent calls are no-ops.
pub fn ensure_started() {
    TX.get_or_init(|| {
        let (tx, rx) = bounded::<WlOverlayCmd>(64);
        thread::Builder::new()
            .name("cua-overlay-wl".into())
            .spawn(move || {
                if let Err(e) = owner_thread(rx) {
                    tracing::warn!("cua-overlay-wl thread exited with error: {e}");
                }
            })
            .expect("spawn cua-overlay-wl thread");
        tx
    });
}

/// Translate a generic [`OverlayMsg`] (the cross-platform command shape) to
/// the subset this overlay path implements. Returns true if the message
/// was forwarded; false if it was silently dropped (richer visual the v1
/// layer-shell path doesn't render yet).
pub fn forward(msg: &OverlayMsg) -> bool {
    let Some(tx) = tx() else { return false };
    match msg {
        OverlayMsg::Remove(k) => {
            let _ = tx.try_send(WlOverlayCmd::Remove { key: k.clone() });
            true
        }
        OverlayMsg::Cmd(kc) => match &kc.cmd {
            OverlayCommand::MoveTo { x, y, .. } | OverlayCommand::SnapTo { x, y, .. } => {
                let _ = tx.try_send(WlOverlayCmd::Position {
                    key: kc.key.clone(),
                    x: *x,
                    y: *y,
                });
                true
            }
            OverlayCommand::SetEnabled(en) => {
                let _ = tx.try_send(WlOverlayCmd::SetEnabled {
                    key: kc.key.clone(),
                    enabled: *en,
                });
                true
            }
            // ClickPulse, SetPressed, SetMotion, SetPalette, PinAbove,
            // SetShape, SetGradient, ShowFocusRect are not rendered by the
            // v1 layer-shell path — drop silently so callers don't see
            // errors.
            _ => false,
        },
    }
}

/// Cleanly stop the owner thread. Tests use this; production code typically
/// lets the thread die at process exit.
pub fn shutdown() {
    if let Some(tx) = tx() {
        let _ = tx.send(WlOverlayCmd::Shutdown);
    }
}

// ── owner thread ─────────────────────────────────────────────────────────

#[derive(Default)]
struct OverlayState {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
    output: Option<WlOutput>,
    output_w: u32,
    output_h: u32,
    surface: Option<WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    configured: bool,
    cursor_x: i32,
    cursor_y: i32,
    enabled: bool,
}

fn owner_thread(rx: Receiver<WlOverlayCmd>) -> anyhow::Result<()> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<OverlayState>();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());

    let mut state = OverlayState {
        enabled: true,
        ..OverlayState::default()
    };
    queue.roundtrip(&mut state)?;
    for _ in 0..3 {
        queue.roundtrip(&mut state)?;
    }

    let compositor = state
        .compositor
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor does not expose wl_compositor"))?;
    let shm = state
        .shm
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor does not expose wl_shm"))?;
    let layer_shell = state
        .layer_shell
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor does not expose zwlr_layer_shell_v1"))?;
    let output = state
        .output
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor exposed no wl_output"))?;

    // Build the layer surface: fullscreen, overlay layer, click-through.
    let surface = compositor.create_surface(&qh, ());
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        Some(&output),
        Layer::Overlay,
        "cua-agent-cursor".to_string(),
        &qh,
        (),
    );
    // Anchor to all four edges = full screen.
    layer_surface.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
    layer_surface.set_size(0, 0);
    layer_surface.set_exclusive_zone(-1);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);

    // Click-through: empty input region.
    let region: WlRegion = compositor.create_region(&qh, ());
    surface.set_input_region(Some(&region));
    region.destroy();

    state.surface = Some(surface);
    state.layer_surface = Some(layer_surface);

    // First commit kicks off the configure handshake.
    if let Some(s) = state.surface.as_ref() {
        s.commit();
    }

    // Wait for the first configure event so we know the output dimensions
    // before drawing.
    for _ in 0..10 {
        queue.roundtrip(&mut state)?;
        if state.configured && state.output_w > 0 && state.output_h > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    if !state.configured {
        anyhow::bail!("layer surface never received configure event");
    }

    // Main loop: process incoming commands, redraw on each.
    redraw(&mut state, &shm, &qh)?;
    queue.roundtrip(&mut state)?;

    loop {
        // Non-blocking command poll, then blocking wayland event drain.
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(WlOverlayCmd::Shutdown) => break,
            Ok(WlOverlayCmd::Position { x, y, .. }) => {
                state.cursor_x = x.round() as i32;
                state.cursor_y = y.round() as i32;
                if state.enabled {
                    redraw(&mut state, &shm, &qh)?;
                }
            }
            Ok(WlOverlayCmd::SetEnabled { enabled, .. }) => {
                state.enabled = enabled;
                redraw(&mut state, &shm, &qh)?;
            }
            Ok(WlOverlayCmd::Remove { .. }) => {
                state.enabled = false;
                redraw(&mut state, &shm, &qh)?;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
        queue.dispatch_pending(&mut state)?;
    }

    if let Some(ls) = state.layer_surface.take() {
        ls.destroy();
    }
    if let Some(s) = state.surface.take() {
        s.destroy();
    }
    queue.roundtrip(&mut state)?;
    Ok(())
}

/// Allocate a wl_shm buffer the size of the layer surface, fill it with
/// transparent background + a single tinted dot at the cursor position,
/// attach + damage + commit. This is the v1 visual — a placeholder for
/// the gradient-arrow renderer in `cursor_overlay::render_state` which
/// will land in a follow-up slice once the wayland-side rasterizer is
/// in place.
fn redraw(state: &mut OverlayState, shm: &WlShm, qh: &QueueHandle<OverlayState>) -> anyhow::Result<()> {
    let Some(surface) = state.surface.as_ref() else { return Ok(()) };
    let w = state.output_w.max(1);
    let h = state.output_h.max(1);
    let stride = w as i32 * 4;
    let size = (stride as usize) * (h as usize);

    // Build a memfd-backed shm pool. Reuses the same anon_shm pattern as
    // the screencopy path in mod.rs.
    let (fd, ptr) = super::anon_shm(size)
        .map_err(|e| anyhow::anyhow!("overlay shm allocation failed: {e}"))?;

    // SAFETY: ptr came from mmap of `size` bytes, lifetime bounded to this
    // function.
    let pixels: &mut [u8] = unsafe { std::slice::from_raw_parts_mut(ptr as *mut u8, size) };

    if state.enabled {
        // Transparent background + an opaque tinted dot at the cursor.
        pixels.fill(0); // ARGB8888 = (0, 0, 0, 0) — fully transparent.
        let radius = 12i32;
        let cx = state.cursor_x.clamp(0, w as i32 - 1);
        let cy = state.cursor_y.clamp(0, h as i32 - 1);
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }
                let px = cx + dx;
                let py = cy + dy;
                if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                    continue;
                }
                let idx = ((py as usize) * (stride as usize)) + ((px as usize) * 4);
                // ARGB8888 little-endian = BGRA in memory: tinted cyan, ~80% alpha.
                pixels[idx] = 0xFF;     // B
                pixels[idx + 1] = 0xCC; // G
                pixels[idx + 2] = 0x33; // R
                pixels[idx + 3] = 0xCC; // A
            }
        }
    } else {
        // Fully transparent — nothing visible.
        pixels.fill(0);
    }

    use std::os::fd::AsFd as _;
    let pool_fd = unsafe { super::borrowed_fd(fd) };
    let pool: WlShmPool = shm.create_pool(pool_fd.as_fd(), size as i32, qh, ());
    let buffer: WlBuffer = pool.create_buffer(
        0,
        w as i32,
        h as i32,
        stride,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );

    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, w as i32, h as i32);
    surface.commit();
    pool.destroy();

    // The buffer is released by the compositor via wl_buffer::release.
    // We can't free the mmap until then; cleanup is best-effort at process
    // exit via OS cleanup of the memfd.
    super::cleanup_mmap(ptr, size, fd);
    Ok(())
}

// ── Wayland Dispatch impls ───────────────────────────────────────────────

impl Dispatch<wl_registry::WlRegistry, ()> for OverlayState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind::<WlCompositor, _, _>(
                        name, version.min(6), qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind::<WlShm, _, _>(name, version.min(1), qh, ()));
                }
                "wl_output" => {
                    if state.output.is_none() {
                        state.output = Some(registry.bind::<WlOutput, _, _>(name, version.min(4), qh, ()));
                    }
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind::<ZwlrLayerShellV1, _, _>(
                        name, version.min(4), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<WlCompositor, ()> for OverlayState {
    fn event(
        _state: &mut Self,
        _: &WlCompositor,
        _: <WlCompositor as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<WlShm, ()> for OverlayState {
    fn event(
        _state: &mut Self,
        _: &WlShm,
        _: <WlShm as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<WlOutput, ()> for OverlayState {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: <WlOutput as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_output;
        if let wl_output::Event::Mode { width, height, .. } = event {
            if width > 0 && height > 0 {
                state.output_w = width as u32;
                state.output_h = height as u32;
            }
        }
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for OverlayState {
    fn event(
        _state: &mut Self,
        _: &ZwlrLayerShellV1,
        _: <ZwlrLayerShellV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for OverlayState {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: <ZwlrLayerSurfaceV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, width, height } = event {
            layer.ack_configure(serial);
            if width > 0 {
                state.output_w = width;
            }
            if height > 0 {
                state.output_h = height;
            }
            state.configured = true;
        }
    }
}

impl Dispatch<WlSurface, ()> for OverlayState {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: <WlSurface as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<WlShmPool, ()> for OverlayState {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: <WlShmPool as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<WlBuffer, ()> for OverlayState {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        _: <WlBuffer as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {}
}

impl Dispatch<WlRegion, ()> for OverlayState {
    fn event(
        _: &mut Self,
        _: &WlRegion,
        _: <WlRegion as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {}
}
