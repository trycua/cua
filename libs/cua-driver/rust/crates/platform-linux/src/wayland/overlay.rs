//! Native-Wayland agent-cursor overlay via `zwlr_layer_shell_v1`.
//!
//! Replaces the X11-only `overlay.rs` render loop on wlroots compositors
//! (sway, labwc, kwin 5.27+, hyprland) by creating a full-screen,
//! click-through, always-on-top `wl_surface` anchored to the first output
//! via `zwlr_layer_shell_v1`. The surface renders the same gradient-arrow
//! cursor as the X11 path by sharing `cursor_overlay::RenderStateCore` —
//! bloom, click-pulse, idle-fade, and motion all work identically.
//!
//! GNOME mutter does not expose `zwlr_layer_shell_v1` — those sessions
//! either fall through to the X11 path (XWayland) or the nested-compositor
//! mode that spawns labwc internally.
//!
//! Architecture mirrors the existing `wayland/persistent_vptr.rs`: one
//! owner thread (`cua-overlay-wl`) holds the wayland Connection +
//! EventQueue + layer surface; commands flow in over a `crossbeam-channel`.
//! The render core is ticked at ~60Hz via a calloop timer so motion +
//! spring physics + click pulse advance smoothly even when no new
//! Position command has arrived.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::thread;
use std::time::Instant;

use crossbeam_channel::{bounded, Receiver, Sender};
use cursor_overlay::{CursorConfig, OverlayCommand, OverlayMsg, CursorKey, RenderStateCore};
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
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

/// Commands the overlay owner thread accepts. The richer commands the
/// cross-platform [`RenderStateCore`] understands (MoveTo, ClickPulse,
/// SetPressed) are forwarded as-is so the layer-shell overlay matches the
/// X11 visual: bloom + animated arrow + click pulse + press ring.
enum WlOverlayCmd {
    Cmd { key: CursorKey, cmd: OverlayCommand },
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

/// Translate a generic [`OverlayMsg`] (the cross-platform command shape)
/// to the layer-shell owner thread. The owner-thread render core consumes
/// every variant the X11 path handles; only `ShowFocusRect` (macOS-only)
/// is silently dropped here.
pub fn forward(msg: &OverlayMsg) -> bool {
    let Some(tx) = tx() else { return false };
    match msg {
        OverlayMsg::Remove(k) => {
            let _ = tx.try_send(WlOverlayCmd::Remove { key: k.clone() });
            true
        }
        OverlayMsg::Cmd(kc) => {
            if matches!(&kc.cmd, OverlayCommand::ShowFocusRect(_)) {
                return false;
            }
            let _ = tx.try_send(WlOverlayCmd::Cmd {
                key: kc.key.clone(),
                cmd: kc.cmd.clone(),
            });
            true
        }
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
    /// Cross-platform render core: position, animation, gradient arrow,
    /// bloom, click pulse, idle-fade. Shared verbatim with the X11 path.
    core: RenderStateCore,
    /// In-flight wl_shm buffers awaiting `wl_buffer.release` from the
    /// compositor. Keyed by `WlBuffer` object id; value is the
    /// `(mmap ptr, mmap size, memfd fd)` triple that must be unmapped +
    /// closed once the compositor signals it's done with the buffer.
    /// Replaces the per-redraw `mem::forget` leak: the previous frame's
    /// memory is reclaimed as soon as the compositor releases it.
    pending_buffers: HashMap<u32, (*mut libc::c_void, usize, i32)>,
}

// SAFETY: the raw pointers in pending_buffers point at mmap regions owned
// exclusively by this thread (the owner thread). OverlayState is never
// shared across threads — wayland-client's EventQueue<State> is !Send so
// it stays pinned to the owner thread. The Send/Sync bounds wayland-client
// requires for State types apply to the struct as a whole, hence the
// explicit assertion.
unsafe impl Send for OverlayState {}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            compositor: None,
            shm: None,
            layer_shell: None,
            output: None,
            output_w: 0,
            output_h: 0,
            surface: None,
            layer_surface: None,
            configured: false,
            core: RenderStateCore::new(CursorConfig::default()),
            pending_buffers: HashMap::new(),
        }
    }
}

fn dbg(msg: &str) {
    if std::env::var_os("CUA_OVERLAY_DEBUG").is_some() {
        eprintln!("[cua-overlay-wl] {msg}");
    }
}

fn owner_thread(rx: Receiver<WlOverlayCmd>) -> anyhow::Result<()> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<OverlayState>();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());

    let mut state = OverlayState::default();
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
    dbg(&format!("configured: w={} h={}", state.output_w, state.output_h));

    // Main loop. Tick the render core at ~60Hz so motion + spring physics
    // + click pulse animate smoothly; redraw every tick when the cursor is
    // visible. Commands arriving via the channel update the render core
    // first, then the next tick paints the result.
    redraw(&mut state, &shm, &qh)?;
    queue.roundtrip(&mut state)?;

    let frame_dur = std::time::Duration::from_millis(16);
    let mut last_tick = Instant::now();
    loop {
        // Drain all pending commands without blocking.
        let mut shutdown = false;
        loop {
            match rx.try_recv() {
                Ok(WlOverlayCmd::Shutdown) => { shutdown = true; break; }
                Ok(WlOverlayCmd::Cmd { cmd, .. }) => {
                    // Seed: if the cursor is still at the off-screen sentinel
                    // `(-200, -200)` from `RenderStateCore::new`, snap to a
                    // point near the MoveTo / SnapTo target so the spring
                    // animation starts on-screen. Mirrors X11 overlay.rs's
                    // `seed_start_if_sentinel` helper — without it, the
                    // spring oscillates around the sentinel and the cursor
                    // never reaches the screen.
                    let seed_target = match &cmd {
                        OverlayCommand::MoveTo { x, y, .. }
                        | OverlayCommand::SnapTo { x, y, .. }
                        | OverlayCommand::ClickPulse { x, y } => Some((*x, *y)),
                        _ => None,
                    };
                    if let Some((tx, ty)) = seed_target {
                        if state.core.pos.0 < -50.0 {
                            const SEED_OFFSET: f64 = 16.0;
                            let sx = (tx - SEED_OFFSET).max(2.0);
                            let sy = (ty - SEED_OFFSET).max(2.0);
                            state.core.pos = (sx, sy);
                        }
                    }
                    // apply_command_base consumes every variant the X11
                    // path handles. `move_to_snap_sentinel` / `click_pulse
                    // _sentinel_only` are both `false` here — same as X11.
                    let _ = state.core.apply_command_base(cmd, false, false);
                }
                Ok(WlOverlayCmd::Remove { .. }) => {
                    // Single-cursor overlay: removing the active cursor
                    // hides it. Multi-cursor wlroots support can layer on
                    // top of this in a follow-up if needed.
                    state.core.visible = false;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => { shutdown = true; break; }
            }
        }
        if shutdown { break; }

        // Tick animation forward and repaint if anything changed.
        let now = Instant::now();
        let dt = now.duration_since(last_tick).as_secs_f64().min(0.05);
        last_tick = now;
        state.core.tick_motion(dt);
        if state.configured {
            redraw(&mut state, &shm, &qh)?;
        }
        queue.dispatch_pending(&mut state)?;

        // Sleep for the remainder of the frame budget so the loop doesn't
        // spin. Channel-driven wakeups would be lower-latency, but layer
        // overlays only need to keep up with display refresh.
        let elapsed = last_tick.elapsed();
        if elapsed < frame_dur {
            std::thread::sleep(frame_dur - elapsed);
        }
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

/// Render one cursor frame into a fresh wl_shm ARGB8888 buffer and attach
/// it to the layer surface.
///
/// Pipeline:
/// 1. Allocate a memfd-backed wl_shm pool sized at output_w × output_h.
/// 2. Paint the cross-platform cursor (bloom + click pulse + gradient
///    arrow) into a `tiny_skia::Pixmap` via `cursor_overlay::paint_cursor`
///    — same call the X11 path uses.
/// 3. Channel-swap RGBA → BGRA into the wl_shm buffer (wl_shm Argb8888
///    is little-endian BGRA in memory). This is the inverse of the swap
///    in `ext_screencopy::encode_buffer_to_png`.
/// 4. Attach + damage + commit on the layer surface.
///
/// When the cursor is hidden (`core.visible == false`, idle-faded, or
/// off-screen sentinel) the pixmap is all zeros — the surface remains
/// transparent and click-through.
fn redraw(state: &mut OverlayState, shm: &WlShm, qh: &QueueHandle<OverlayState>) -> anyhow::Result<()> {
    let Some(surface) = state.surface.as_ref() else { return Ok(()) };
    let w = state.output_w.max(1);
    let h = state.output_h.max(1);
    let stride = w as i32 * 4;
    let size = (stride as usize) * (h as usize);

    // Reuses the same anon_shm pattern as the screencopy path in mod.rs.
    let (fd, ptr) = super::anon_shm(size)
        .map_err(|e| anyhow::anyhow!("overlay shm allocation failed: {e}"))?;

    // SAFETY: ptr came from mmap of `size` bytes, lifetime bounded to this
    // function.
    let pixels: &mut [u8] = unsafe { std::slice::from_raw_parts_mut(ptr as *mut u8, size) };

    // Paint the cursor into a tiny_skia pixmap. paint_cursor early-returns
    // when the cursor is hidden / off-screen / idle-faded, so the pixmap
    // is left fully transparent in those cases (which is also what we want
    // for the click-through layer surface).
    let mut pm = tiny_skia::Pixmap::new(w, h)
        .unwrap_or_else(|| tiny_skia::Pixmap::new(1, 1).unwrap());
    // backing_scale=1.0 matches the X11 path; per-output Wayland scale is
    // a follow-up (would consume `wl_output.scale` and `preferred_buffer
    // _scale` from wl_surface v6).
    cursor_overlay::paint_cursor(&mut pm, &state.core, 0.0, 0.0, None, 1.0);

    // CUA_OVERLAY_DEBUG=1 paints a 60x60 magenta square at the cursor's
    // current pos on top of the gradient arrow. Useful when validating
    // layer-shell visibility on a new compositor — the gradient arrow is
    // small at native scale and easy to miss in a screenshot, while the
    // magenta block is impossible to miss.
    if std::env::var_os("CUA_OVERLAY_DEBUG").is_some() {
        let (cx, cy) = state.core.pos;
        let cx = cx as i32;
        let cy = cy as i32;
        let half = 30i32;
        for dy in -half..half {
            for dx in -half..half {
                let px = cx + dx;
                let py = cy + dy;
                if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                    continue;
                }
                let off = ((py as usize) * (w as usize) + (px as usize)) * 4;
                pm.data_mut()[off] = 0xFF;     // R
                pm.data_mut()[off + 1] = 0x00; // G
                pm.data_mut()[off + 2] = 0xFF; // B
                pm.data_mut()[off + 3] = 0xFF; // A
            }
        }
    }

// RGBA → BGRA channel swap. tiny_skia stores pixels as RGBA8888
    // (premultiplied); wl_shm Argb8888 is little-endian = BGRA in memory.
    // Mirrors the inverse swap in ext_screencopy::encode_buffer_to_png.
    let src = pm.data();
    for i in (0..size).step_by(4) {
        // pm.data() is already RGBA premultiplied; just swap R↔B.
        pixels[i] = src[i + 2];     // B ← R
        pixels[i + 1] = src[i + 1]; // G
        pixels[i + 2] = src[i];     // R ← B
        pixels[i + 3] = src[i + 3]; // A
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

    // Track the (mmap, fd) by buffer object id so the wl_buffer.release
    // event Dispatch handler can clean up exactly when the compositor is
    // done with the underlying memory — no leak, no use-after-free.
    let buffer_id = buffer.id().protocol_id();
    state.pending_buffers.insert(buffer_id, (ptr, size, fd));

    dbg(&format!("redraw w={w} h={h} stride={stride} buf_id={buffer_id} pos=({:.1},{:.1}) visible={}", state.core.pos.0, state.core.pos.1, state.core.visible));
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, w as i32, h as i32);
    surface.commit();
    pool.destroy();
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
        state: &mut Self,
        buffer: &WlBuffer,
        event: <WlBuffer as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_buffer;
        if matches!(event, wl_buffer::Event::Release) {
            // Compositor is done with the underlying mmap. Free it +
            // close the memfd + destroy the wayland object.
            let id = buffer.id().protocol_id();
            if let Some((ptr, size, fd)) = state.pending_buffers.remove(&id) {
                super::cleanup_mmap(ptr, size, fd);
            }
            buffer.destroy();
        }
    }
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
