//! Native-Wayland agent-cursor overlay via `zwlr_layer_shell_v1`.
//!
//! Replaces the X11-only `overlay.rs` render loop on wlroots compositors
//! (sway, labwc, kwin 5.27+, hyprland) by creating a full-screen,
//! click-through, always-on-top `wl_surface` on every enabled output
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
//! The render core wakes at frame cadence only while pixels can change;
//! stable or hidden state performs only a cheap, one-second topology check.

use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use cursor_overlay::{CursorConfig, OverlayCommand, OverlayMsg, RenderStateCore};
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
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

/// Commands the overlay owner thread accepts. The richer commands the
/// cross-platform [`RenderStateCore`] understands (MoveTo, ClickPulse,
/// SetPressed) are forwarded as-is so the layer-shell overlay matches the
/// X11 visual: bloom + animated arrow + click pulse + press ring.
enum WlOverlayCmd {
    Cmd { cmd: OverlayCommand },
    Remove,
    Shutdown,
}

static TX: OnceLock<Sender<WlOverlayCmd>> = OnceLock::new();
// Starts false deliberately: the platform registration path must explicitly
// opt the native Wayland overlay in with the daemon's CursorConfig. This keeps
// lazy forwarding from bypassing --no-overlay before any window exists.
static CONFIG_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_config_enabled(enabled: bool) {
    CONFIG_ENABLED.store(enabled, Ordering::Release);
}

fn tx() -> Option<&'static Sender<WlOverlayCmd>> {
    TX.get()
}

/// Lazily start the owner thread. Idempotent — safe to call from every
/// MCP tool invocation; subsequent calls are no-ops.
pub fn ensure_started() -> bool {
    if !CONFIG_ENABLED.load(Ordering::Acquire) {
        return false;
    }
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
    true
}

/// Translate a generic [`OverlayMsg`] (the cross-platform command shape)
/// to the layer-shell owner thread. The owner-thread render core consumes
/// every variant the X11 path handles; only `ShowFocusRect` (macOS-only)
/// is silently dropped here.
pub fn forward(msg: &OverlayMsg) -> bool {
    if !should_forward(CONFIG_ENABLED.load(Ordering::Acquire), msg) {
        return false;
    }
    // The native Wayland path owns one cursor and does not keep keyed session
    // tombstones. Accept revival without starting the compositor thread.
    if matches!(msg, OverlayMsg::Revive(_)) {
        return true;
    }
    // Lazy startup: spawning the layer-shell owner thread + connecting to
    // the Wayland compositor takes 100-300ms. Doing that at cua-driver mcp
    // boot (the old eager-init path) was tipping the borderline CI
    // cursor-click-gif test over its 20s budget. ensure_started is
    // idempotent so calling it on every forward is fine — the OnceLock
    // bypasses the spawn after the first call.
    if !ensure_started() {
        return false;
    }
    let Some(tx) = tx() else { return false };
    match msg {
        OverlayMsg::Remove(k) => {
            let _ = k;
            let _ = tx.try_send(WlOverlayCmd::Remove);
            true
        }
        OverlayMsg::Cmd(kc) => {
            if matches!(&kc.cmd, OverlayCommand::ShowFocusRect(_)) {
                return false;
            }
            let _ = tx.try_send(WlOverlayCmd::Cmd {
                cmd: kc.cmd.clone(),
            });
            true
        }
        OverlayMsg::Revive(_) => true,
    }
}

fn should_forward(config_enabled: bool, msg: &OverlayMsg) -> bool {
    config_enabled
        && !matches!(
            msg,
            OverlayMsg::Cmd(kc) if matches!(&kc.cmd, OverlayCommand::ShowFocusRect(_))
        )
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
    xdg_output_manager: Option<ZxdgOutputManagerV1>,
    outputs: HashMap<u32, NativeOutput>,
    /// Outputs whose most recently committed buffer contains cursor pixels.
    /// Usually this contains exactly one output; keeping a set makes hide,
    /// removal, and topology changes clear every previously painted surface.
    painted_outputs: HashSet<u32>,
    topology_dirty: bool,
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

struct NativeOutput {
    output: WlOutput,
    xdg_output: Option<ZxdgOutputV1>,
    surface: Option<WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    wl_origin: Option<(i32, i32)>,
    logical_origin: Option<(i32, i32)>,
    logical_size: Option<(u32, u32)>,
    configured_size: Option<(u32, u32)>,
    mode_size: Option<(u32, u32)>,
    scale: i32,
    name: Option<String>,
    closed: bool,
}

impl NativeOutput {
    fn new(output: WlOutput) -> Self {
        Self {
            output,
            xdg_output: None,
            surface: None,
            layer_surface: None,
            wl_origin: None,
            logical_origin: None,
            logical_size: None,
            configured_size: None,
            mode_size: None,
            scale: 1,
            name: None,
            closed: false,
        }
    }

    fn layout(&self, id: u32) -> Option<OutputLayout> {
        if self.layer_surface.is_none() || self.configured_size.is_none() {
            return None;
        }
        let (origin_x, origin_y) = self.logical_origin.or(self.wl_origin).unwrap_or((0, 0));
        let (width, height) = self.logical_size.or(self.configured_size).or_else(|| {
            let scale = self.scale.max(1) as u32;
            self.mode_size
                .map(|(width, height)| ((width / scale).max(1), (height / scale).max(1)))
        })?;
        (width > 0 && height > 0).then_some(OutputLayout {
            id,
            origin_x,
            origin_y,
            width,
            height,
        })
    }

    fn destroy_surfaces(&mut self) {
        self.close_layer();
        if let Some(xdg_output) = self.xdg_output.take() {
            xdg_output.destroy();
        }
    }

    fn close_layer(&mut self) {
        if let Some(layer_surface) = self.layer_surface.take() {
            layer_surface.destroy();
        }
        if let Some(surface) = self.surface.take() {
            surface.destroy();
        }
        self.configured_size = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OutputLayout {
    id: u32,
    origin_x: i32,
    origin_y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SelectedOutput {
    id: u32,
    local_x: f64,
    local_y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameTarget {
    id: u32,
    paint_cursor: bool,
}

#[derive(Clone, Copy)]
struct OutputData {
    id: u32,
}

#[derive(Clone, Copy)]
struct LayerData {
    id: u32,
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
            xdg_output_manager: None,
            outputs: HashMap::new(),
            painted_outputs: HashSet::new(),
            topology_dirty: false,
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

fn select_output(layouts: &[OutputLayout], x: f64, y: f64) -> Option<SelectedOutput> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    layouts
        .iter()
        .filter(|layout| {
            x >= f64::from(layout.origin_x)
                && y >= f64::from(layout.origin_y)
                && x < f64::from(layout.origin_x) + f64::from(layout.width)
                && y < f64::from(layout.origin_y) + f64::from(layout.height)
        })
        .min_by_key(|layout| layout.id)
        .map(|layout| SelectedOutput {
            id: layout.id,
            local_x: x - f64::from(layout.origin_x),
            local_y: y - f64::from(layout.origin_y),
        })
}

fn frame_plan(
    layouts: &[OutputLayout],
    painted_outputs: &HashSet<u32>,
    cursor_position: Option<(f64, f64)>,
) -> (Option<SelectedOutput>, Vec<FrameTarget>) {
    let selected = cursor_position.and_then(|(x, y)| select_output(layouts, x, y));
    let mut ids: Vec<u32> = painted_outputs.iter().copied().collect();
    if let Some(selected) = selected {
        if !ids.contains(&selected.id) {
            ids.push(selected.id);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    let targets = ids
        .into_iter()
        .map(|id| FrameTarget {
            id,
            paint_cursor: selected.is_some_and(|selected| selected.id == id),
        })
        .collect();
    (selected, targets)
}

fn ensure_output_resources(state: &mut OverlayState, qh: &QueueHandle<OverlayState>) {
    let ids: Vec<u32> = state.outputs.keys().copied().collect();
    for id in ids {
        ensure_xdg_output(state, id, qh);
        ensure_layer_surface(state, id, qh);
    }
}

fn ensure_xdg_output(state: &mut OverlayState, id: u32, qh: &QueueHandle<OverlayState>) {
    let Some(manager) = state.xdg_output_manager.clone() else {
        return;
    };
    let Some(output) = state.outputs.get_mut(&id) else {
        return;
    };
    if output.xdg_output.is_none() {
        output.xdg_output = Some(manager.get_xdg_output(&output.output, qh, OutputData { id }));
    }
}

fn ensure_layer_surface(state: &mut OverlayState, id: u32, qh: &QueueHandle<OverlayState>) {
    let (Some(compositor), Some(layer_shell)) =
        (state.compositor.clone(), state.layer_shell.clone())
    else {
        return;
    };
    let Some(output) = state.outputs.get_mut(&id) else {
        return;
    };
    if output.layer_surface.is_some() || output.closed {
        return;
    }

    let surface = compositor.create_surface(qh, ());
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        Some(&output.output),
        Layer::Overlay,
        "cua-agent-cursor".to_string(),
        qh,
        LayerData { id },
    );
    layer_surface.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
    layer_surface.set_size(0, 0);
    layer_surface.set_exclusive_zone(-1);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);

    let region: WlRegion = compositor.create_region(qh, ());
    surface.set_input_region(Some(&region));
    region.destroy();

    surface.commit();
    output.surface = Some(surface);
    output.layer_surface = Some(layer_surface);
}

fn owner_thread(rx: Receiver<WlOverlayCmd>) -> anyhow::Result<()> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<OverlayState>();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());

    let mut state = OverlayState::default();
    queue.roundtrip(&mut state)?;

    state
        .compositor
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor does not expose wl_compositor"))?;
    let shm = state
        .shm
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor does not expose wl_shm"))?;
    state
        .layer_shell
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor does not expose zwlr_layer_shell_v1"))?;
    if state.outputs.is_empty() {
        anyhow::bail!("compositor exposed no wl_output");
    }

    // Build one fullscreen, click-through layer surface per advertised output.
    ensure_output_resources(&mut state, &qh);

    // Give every enabled output a chance to configure. Disabled outputs may
    // stay advertised without configuring and are excluded from selection.
    for _ in 0..10 {
        queue.roundtrip(&mut state)?;
        ensure_output_resources(&mut state, &qh);
        if state
            .outputs
            .values()
            .filter(|output| output.layer_surface.is_some())
            .all(|output| output.configured_size.is_some())
        {
            break;
        }
    }

    let configured_outputs = state
        .outputs
        .values()
        .filter(|output| output.configured_size.is_some())
        .count();
    if configured_outputs == 0 {
        anyhow::bail!("no layer surface received a configure event");
    }
    dbg(&format!("configured outputs: {configured_outputs}"));
    state.topology_dirty = false;

    // Demand-driven main loop. Stable cursors perform only a cheap Wayland
    // maintenance roundtrip once per second so output topology changes are
    // observed; full-display SHM work remains limited to visual changes.
    let mut last_tick = Instant::now();
    let mut frame_tick_needed = false;
    loop {
        let wait = next_wait(&state.core, frame_tick_needed, state.topology_dirty);
        let (first_cmd, timed_out) = match wait_for_work(&rx, wait) {
            WlWake::Command(cmd) => (Some(cmd), None),
            WlWake::Timeout => (None, Some(wait)),
            WlWake::Disconnected => break,
        };

        let now = Instant::now();
        let elapsed = now.duration_since(last_tick).as_secs_f64();
        last_tick = now;

        let mut dirty = false;
        let mut shutdown = false;
        let mut pending = first_cmd;
        loop {
            let received = pending.take().map(Ok).unwrap_or_else(|| rx.try_recv());
            match received {
                Ok(WlOverlayCmd::Shutdown) => {
                    shutdown = true;
                    break;
                }
                Ok(WlOverlayCmd::Cmd { cmd }) => {
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
                    let disabling = matches!(&cmd, OverlayCommand::SetEnabled(false));
                    dirty |= state.core.apply_command_base(cmd, false, false);
                    if disabling {
                        quiesce_hidden(&mut state.core);
                    }
                }
                Ok(WlOverlayCmd::Remove) => {
                    // Single-cursor overlay: removing the active cursor
                    // hides it. Multi-cursor wlroots support can layer on
                    // top of this in a follow-up if needed.
                    dirty |= state.core.visible || state.core.pos.0 >= -100.0;
                    state.core.visible = false;
                    quiesce_hidden(&mut state.core);
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    shutdown = true;
                    break;
                }
            }
        }
        if shutdown {
            break;
        }

        // Advance only on a scheduled animation/fade wake. A command wake
        // applies the new state at dt=0, avoiding a jump proportional to how
        // long the loop was parked.
        if let Some(timeout_kind) = timed_out {
            match timeout_kind {
                WlWait::Frame => {
                    state.core.tick_motion(elapsed.min(0.05));
                    dirty = true;
                }
                WlWait::Deadline(_) => {
                    state.core.tick_motion(elapsed);
                    dirty = true;
                }
                WlWait::Maintenance(_) => {
                    let idle_alpha = state.core.idle_alpha;
                    state.core.tick_motion(elapsed);
                    dirty |= state.core.idle_alpha != idle_alpha || needs_frame_tick(&state.core);

                    let topology_was_dirty = state.topology_dirty;
                    state.topology_dirty = false;
                    queue.roundtrip(&mut state)?;
                    ensure_output_resources(&mut state, &qh);
                    dirty |= topology_was_dirty || state.topology_dirty;
                    state.topology_dirty = false;
                }
            }
        }
        let next_frame_tick_needed = needs_frame_tick(&state.core);
        if dirty || frame_tick_needed || next_frame_tick_needed {
            redraw(&mut state, &shm, &qh)?;
            // Flush the committed frame and dispatch wl_buffer.release before
            // parking. The buffer map remains authoritative until release, so
            // no mmap/fd can be reclaimed while the compositor still uses it.
            queue.roundtrip(&mut state)?;
        }
        frame_tick_needed = next_frame_tick_needed;
    }

    for output in state.outputs.values_mut() {
        output.destroy_surfaces();
    }
    queue.roundtrip(&mut state)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WlWait {
    Frame,
    Deadline(Duration),
    Maintenance(Duration),
}

enum WlWake {
    Command(WlOverlayCmd),
    Timeout,
    Disconnected,
}

fn wait_for_work(rx: &Receiver<WlOverlayCmd>, wait: WlWait) -> WlWake {
    match wait {
        WlWait::Frame => match rx.recv_timeout(Duration::from_millis(16)) {
            Ok(cmd) => WlWake::Command(cmd),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => WlWake::Timeout,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => WlWake::Disconnected,
        },
        WlWait::Deadline(timeout) | WlWait::Maintenance(timeout) => {
            match rx.recv_timeout(timeout) {
                Ok(cmd) => WlWake::Command(cmd),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => WlWake::Timeout,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => WlWake::Disconnected,
            }
        }
    }
}

const TOPOLOGY_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(1);

fn next_wait(core: &RenderStateCore, frame_tick_needed: bool, topology_dirty: bool) -> WlWait {
    if topology_dirty {
        return WlWait::Maintenance(Duration::ZERO);
    }
    if frame_tick_needed || needs_frame_tick(core) {
        return WlWait::Frame;
    }
    match idle_fade_wait(core) {
        Some(wait) if wait <= TOPOLOGY_MAINTENANCE_INTERVAL => WlWait::Deadline(wait),
        _ => WlWait::Maintenance(TOPOLOGY_MAINTENANCE_INTERVAL),
    }
}

fn needs_frame_tick(core: &RenderStateCore) -> bool {
    if !core.visible || core.pos.0 < -100.0 {
        return false;
    }
    let fade_start = core.motion.idle_hide_ms / 1000.0;
    core.path.is_some()
        || core.spring.is_some()
        || core.click_t.is_some()
        || core.session_badge_needs_frame_tick()
        || (core.motion.idle_hide_ms > 0.0
            && core.idle_secs >= fade_start
            && core.idle_alpha >= 0.004)
}

fn idle_fade_wait(core: &RenderStateCore) -> Option<Duration> {
    if !core.visible
        || core.pos.0 < -100.0
        || core.motion.idle_hide_ms <= 0.0
        || core.path.is_some()
        || core.spring.is_some()
        || core.click_t.is_some()
    {
        return None;
    }
    let remaining = core.motion.idle_hide_ms / 1000.0 - core.idle_secs;
    (remaining.is_finite() && remaining > 0.0).then(|| Duration::from_secs_f64(remaining))
}

fn quiesce_hidden(core: &mut RenderStateCore) {
    core.path = None;
    core.spring = None;
    core.spring_tgt = None;
    core.click_t = None;
}

/// Render the selected output plus a transparent clearing frame on every
/// previously painted output the cursor has left.
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
fn redraw(
    state: &mut OverlayState,
    shm: &WlShm,
    qh: &QueueHandle<OverlayState>,
) -> anyhow::Result<()> {
    let layouts: Vec<OutputLayout> = state
        .outputs
        .iter()
        .filter_map(|(&id, output)| output.layout(id))
        .collect();
    let cursor_position =
        (state.core.visible && state.core.pos.0 >= -100.0 && state.core.idle_alpha >= 0.004)
            .then_some(state.core.pos);
    let (selected, targets) = frame_plan(&layouts, &state.painted_outputs, cursor_position);

    for target in targets {
        redraw_output(state, shm, qh, target)?;
    }

    state.painted_outputs.clear();
    if let Some(selected) = selected {
        state.painted_outputs.insert(selected.id);
    }
    Ok(())
}

fn redraw_output(
    state: &mut OverlayState,
    shm: &WlShm,
    qh: &QueueHandle<OverlayState>,
    target: FrameTarget,
) -> anyhow::Result<()> {
    let Some((surface, layout)) = state
        .outputs
        .get(&target.id)
        .and_then(|output| Some((output.surface.clone()?, output.layout(target.id)?)))
    else {
        return Ok(());
    };
    let w = layout.width.max(1);
    let h = layout.height.max(1);
    let stride = w
        .checked_mul(4)
        .and_then(|stride| i32::try_from(stride).ok())
        .ok_or_else(|| anyhow::anyhow!("overlay output {w}x{h} has an invalid stride"))?;
    let size = usize::try_from(stride)
        .ok()
        .and_then(|stride| stride.checked_mul(h as usize))
        .ok_or_else(|| anyhow::anyhow!("overlay output {w}x{h} buffer size overflow"))?;

    // Reuses the same anon_shm pattern as the screencopy path in mod.rs.
    let (fd, ptr) =
        super::anon_shm(size).map_err(|e| anyhow::anyhow!("overlay shm allocation failed: {e}"))?;

    // SAFETY: ptr came from mmap of `size` bytes, lifetime bounded to this
    // function.
    let pixels: &mut [u8] = unsafe { std::slice::from_raw_parts_mut(ptr as *mut u8, size) };

    // A clearing target intentionally stays transparent. The selected target
    // subtracts its logical compositor origin from the global cursor position;
    // this is the same origin contract used by the Windows virtual desktop.
    let pm_result = tiny_skia::Pixmap::new(w, h);
    let mut pm = match pm_result {
        Some(p) => p,
        // tiny_skia::Pixmap::new only fails on OOM at sizes that fit u32.
        // We refuse to fall back to a 1x1 pixmap because the subsequent
        // RGBA → BGRA loop indexes `src[i+3]` over the full `size` range —
        // a 1x1 source would crash. Surface the allocation failure
        // properly instead.
        None => anyhow::bail!(
            "tiny_skia::Pixmap::new({w}, {h}) failed — out of memory for the overlay buffer"
        ),
    };
    if target.paint_cursor {
        cursor_overlay::paint_cursor(
            &mut pm,
            &state.core,
            f64::from(layout.origin_x),
            f64::from(layout.origin_y),
            None,
            1.0,
        );
    }

    // CUA_OVERLAY_DEBUG=1 paints a 60x60 magenta square at the cursor's
    // current pos on top of the gradient arrow. Useful when validating
    // layer-shell visibility on a new compositor — the gradient arrow is
    // small at native scale and easy to miss in a screenshot, while the
    // magenta block is impossible to miss.
    if target.paint_cursor && std::env::var_os("CUA_OVERLAY_DEBUG").is_some() {
        let (cx, cy) = state.core.pos;
        let cx = (cx - f64::from(layout.origin_x)) as i32;
        let cy = (cy - f64::from(layout.origin_y)) as i32;
        let half = 30i32;
        for dy in -half..half {
            for dx in -half..half {
                let px = cx + dx;
                let py = cy + dy;
                if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                    continue;
                }
                let off = ((py as usize) * (w as usize) + (px as usize)) * 4;
                pm.data_mut()[off] = 0xFF; // R
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
        pixels[i] = src[i + 2]; // B ← R
        pixels[i + 1] = src[i + 1]; // G
        pixels[i + 2] = src[i]; // R ← B
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

    dbg(&format!(
        "redraw output={} origin=({}, {}) w={w} h={h} stride={stride} buf_id={buffer_id} pos=({:.1},{:.1}) paint={}",
        output_label(state, target.id),
        layout.origin_x,
        layout.origin_y,
        state.core.pos.0,
        state.core.pos.1,
        target.paint_cursor,
    ));
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, w as i32, h as i32);
    surface.commit();
    pool.destroy();
    Ok(())
}

fn output_label(state: &OverlayState, id: u32) -> String {
    state
        .outputs
        .get(&id)
        .and_then(|output| output.name.clone())
        .unwrap_or_else(|| format!("wl_output#{id}"))
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
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                match interface.as_str() {
                    "wl_compositor" => {
                        state.compositor =
                            Some(registry.bind::<WlCompositor, _, _>(name, version.min(6), qh, ()));
                    }
                    "wl_shm" => {
                        state.shm =
                            Some(registry.bind::<WlShm, _, _>(name, version.min(1), qh, ()));
                    }
                    "wl_output" => {
                        let output = registry.bind::<WlOutput, _, _>(
                            name,
                            version.min(4),
                            qh,
                            OutputData { id: name },
                        );
                        state.outputs.insert(name, NativeOutput::new(output));
                        state.topology_dirty = true;
                    }
                    "zwlr_layer_shell_v1" => {
                        state.layer_shell = Some(registry.bind::<ZwlrLayerShellV1, _, _>(
                            name,
                            version.min(4),
                            qh,
                            (),
                        ));
                    }
                    "zxdg_output_manager_v1" => {
                        state.xdg_output_manager =
                            Some(registry.bind::<ZxdgOutputManagerV1, _, _>(
                                name,
                                version.min(3),
                                qh,
                                (),
                            ));
                    }
                    _ => {}
                }
                ensure_output_resources(state, qh);
            }
            wl_registry::Event::GlobalRemove { name } => {
                state.painted_outputs.remove(&name);
                if let Some(mut output) = state.outputs.remove(&name) {
                    output.destroy_surfaces();
                    // wl_output.release was added in v3. On an older generic
                    // wlroots compositor, dropping the client proxy is the only
                    // valid cleanup; issuing the newer request would be a
                    // protocol error.
                    if output.output.version() >= 3 {
                        output.output.release();
                    }
                    state.topology_dirty = true;
                }
            }
            _ => {}
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
    ) {
    }
}

impl Dispatch<WlShm, ()> for OverlayState {
    fn event(
        _state: &mut Self,
        _: &WlShm,
        _: <WlShm as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, OutputData> for OverlayState {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: <WlOutput as wayland_client::Proxy>::Event,
        data: &OutputData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_output;
        let Some(output) = state.outputs.get_mut(&data.id) else {
            return;
        };
        match event {
            wl_output::Event::Geometry { x, y, .. } => {
                state.topology_dirty |= output.wl_origin != Some((x, y));
                output.wl_origin = Some((x, y));
            }
            wl_output::Event::Mode { width, height, .. } if width > 0 && height > 0 => {
                state.topology_dirty |= output.mode_size != Some((width as u32, height as u32));
                output.mode_size = Some((width as u32, height as u32));
            }
            wl_output::Event::Scale { factor } => {
                state.topology_dirty |= output.scale != factor.max(1);
                output.scale = factor.max(1);
            }
            wl_output::Event::Name { name } => {
                output.name = Some(name);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputManagerV1, ()> for OverlayState {
    fn event(
        _: &mut Self,
        _: &ZxdgOutputManagerV1,
        _: <ZxdgOutputManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZxdgOutputV1, OutputData> for OverlayState {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: <ZxdgOutputV1 as Proxy>::Event,
        data: &OutputData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.get_mut(&data.id) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                state.topology_dirty |= output.logical_origin != Some((x, y));
                output.logical_origin = Some((x, y));
            }
            zxdg_output_v1::Event::LogicalSize { width, height } if width > 0 && height > 0 => {
                state.topology_dirty |= output.logical_size != Some((width as u32, height as u32));
                output.logical_size = Some((width as u32, height as u32));
            }
            zxdg_output_v1::Event::Name { name } => {
                output.name = Some(name);
            }
            _ => {}
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
    ) {
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, LayerData> for OverlayState {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: <ZwlrLayerSurfaceV1 as wayland_client::Proxy>::Event,
        data: &LayerData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                if let Some(output) = state.outputs.get_mut(&data.id) {
                    output.closed = false;
                    let fallback = output.logical_size.or(output.mode_size).unwrap_or((1, 1));
                    let configured_size = (
                        if width > 0 { width } else { fallback.0 },
                        if height > 0 { height } else { fallback.1 },
                    );
                    state.topology_dirty |= output.configured_size != Some(configured_size);
                    output.configured_size = Some(configured_size);
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.painted_outputs.remove(&data.id);
                if let Some(output) = state.outputs.get_mut(&data.id) {
                    output.closed = true;
                    output.close_layer();
                    state.topology_dirty = true;
                }
            }
            _ => {}
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
    ) {
    }
}

impl Dispatch<WlShmPool, ()> for OverlayState {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: <WlShmPool as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
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
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cursor_overlay::KeyedOverlayCommand;

    fn message(cmd: OverlayCommand) -> OverlayMsg {
        OverlayMsg::Cmd(KeyedOverlayCommand {
            key: "test".to_owned(),
            cmd,
        })
    }

    fn positioned_core() -> RenderStateCore {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.pos = (100.0, 100.0);
        core.motion.idle_hide_ms = 1_000.0;
        core
    }

    fn three_monitor_layout() -> Vec<OutputLayout> {
        vec![
            // 1920x1080 logical laptop panel at 2x backing scale.
            OutputLayout {
                id: 1,
                origin_x: 0,
                origin_y: 0,
                width: 1920,
                height: 1080,
            },
            // 2560x1440 logical external display, raised above the laptop.
            OutputLayout {
                id: 2,
                origin_x: 1920,
                origin_y: -200,
                width: 2560,
                height: 1440,
            },
            // Fractionally scaled portrait display left of the laptop.
            OutputLayout {
                id: 3,
                origin_x: -1280,
                origin_y: 56,
                width: 1280,
                height: 1024,
            },
        ]
    }

    #[test]
    fn selects_output_and_converts_global_to_output_local_coordinates() {
        let layouts = three_monitor_layout();
        assert_eq!(
            select_output(&layouts, 2500.0, 100.0),
            Some(SelectedOutput {
                id: 2,
                local_x: 580.0,
                local_y: 300.0,
            })
        );
        assert_eq!(
            select_output(&layouts, -1000.0, 256.0),
            Some(SelectedOutput {
                id: 3,
                local_x: 280.0,
                local_y: 200.0,
            })
        );
        assert_eq!(
            select_output(&layouts, 1919.0, 500.0).map(|output| output.id),
            Some(1)
        );
        assert_eq!(
            select_output(&layouts, 1920.0, 500.0).map(|output| output.id),
            Some(2)
        );
        assert_eq!(select_output(&layouts, 5000.0, 5000.0), None);
    }

    #[test]
    fn crossing_outputs_clears_the_old_surface_and_paints_the_new_one() {
        let painted = HashSet::from([1]);
        let (selected, targets) =
            frame_plan(&three_monitor_layout(), &painted, Some((2500.0, 100.0)));

        assert_eq!(selected.map(|output| output.id), Some(2));
        assert_eq!(
            targets,
            vec![
                FrameTarget {
                    id: 1,
                    paint_cursor: false,
                },
                FrameTarget {
                    id: 2,
                    paint_cursor: true,
                },
            ]
        );
    }

    #[test]
    fn hide_or_session_removal_clears_every_previously_painted_output() {
        let painted = HashSet::from([1, 2, 3]);
        let (selected, targets) = frame_plan(&three_monitor_layout(), &painted, None);

        assert!(selected.is_none());
        assert_eq!(targets.len(), 3);
        assert!(targets.iter().all(|target| !target.paint_cursor));
        assert_eq!(
            targets.iter().map(|target| target.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn fresh_sentinel_overlay_uses_only_topology_maintenance() {
        let core = RenderStateCore::new(CursorConfig::default());
        assert_eq!(
            next_wait(&core, false, false),
            WlWait::Maintenance(TOPOLOGY_MAINTENANCE_INTERVAL)
        );
        assert!(!needs_frame_tick(&core));
        assert_eq!(
            next_wait(&core, false, true),
            WlWait::Maintenance(Duration::ZERO)
        );
        assert!(frame_plan(&three_monitor_layout(), &HashSet::new(), None)
            .1
            .is_empty());
    }

    #[test]
    fn stable_visible_overlay_sleeps_until_idle_fade_deadline() {
        let mut core = positioned_core();
        core.idle_secs = 0.25;
        assert_eq!(
            next_wait(&core, false, false),
            WlWait::Deadline(Duration::from_millis(750))
        );
        assert!(!needs_frame_tick(&core));
    }

    #[test]
    fn animation_and_fade_use_frame_cadence() {
        let mut core = positioned_core();
        core.click_t = Some(0.0);
        assert!(needs_frame_tick(&core));
        assert_eq!(next_wait(&core, false, false), WlWait::Frame);

        core.click_t = None;
        core.idle_secs = 1.0;
        core.idle_alpha = 1.0;
        assert!(needs_frame_tick(&core));
        assert_eq!(next_wait(&core, false, false), WlWait::Frame);
    }

    #[test]
    fn hidden_and_disabled_state_quiesces_deterministically() {
        let mut core = positioned_core();
        core.click_t = Some(0.25);
        core.visible = false;
        quiesce_hidden(&mut core);
        assert!(core.click_t.is_none());
        assert!(core.path.is_none());
        assert!(core.spring.is_none());
        assert_eq!(
            next_wait(&core, false, false),
            WlWait::Maintenance(TOPOLOGY_MAINTENANCE_INTERVAL)
        );
    }

    #[test]
    fn maintenance_scheduler_wakes_immediately_on_command_arrival() {
        let (tx, rx) = bounded(1);
        tx.send(WlOverlayCmd::Remove).unwrap();
        assert!(matches!(
            wait_for_work(&rx, WlWait::Maintenance(Duration::from_secs(1))),
            WlWake::Command(WlOverlayCmd::Remove)
        ));
    }

    #[test]
    fn disabled_config_refuses_forwarding_and_thread_startup() {
        set_config_enabled(false);
        let msg = message(OverlayCommand::SnapTo {
            x: 10.0,
            y: 20.0,
            heading_radians: None,
        });
        let had_thread = TX.get().is_some();
        assert!(!should_forward(false, &msg));
        assert!(!ensure_started());
        assert_eq!(TX.get().is_some(), had_thread);
    }
}
