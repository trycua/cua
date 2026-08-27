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
use cursor_overlay::{CursorConfig, CursorKey, OverlayCommand, OverlayMsg, RenderStateCore};
use wayland_client::{
    globals::{registry_queue_init, GlobalListContents},
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
    Cmd { key: CursorKey, cmd: OverlayCommand },
    Remove(CursorKey),
    Revive(CursorKey),
    Shutdown,
}

static TX: OnceLock<Sender<WlOverlayCmd>> = OnceLock::new();
// Starts false deliberately: the platform registration path must explicitly
// opt the native Wayland overlay in with the daemon's CursorConfig. This keeps
// lazy forwarding from bypassing --no-overlay before any window exists.
static CONFIG_ENABLED: AtomicBool = AtomicBool::new(false);
static CONFIG_TEMPLATE: OnceLock<CursorConfig> = OnceLock::new();
static LAYER_SHELL_AVAILABLE: OnceLock<bool> = OnceLock::new();

struct LayerShellProbe;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for LayerShellProbe {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Probe once so a queued command is never mistaken for a usable layer-shell
/// backend. This lets an older compositor helper remain a real fallback when
/// the compositor does not advertise `zwlr_layer_shell_v1`.
pub fn available() -> bool {
    *LAYER_SHELL_AVAILABLE.get_or_init(|| {
        let Ok(conn) = Connection::connect_to_env() else {
            return false;
        };
        let Ok((globals, _queue)) = registry_queue_init::<LayerShellProbe>(&conn) else {
            return false;
        };
        globals.contents().with_list(|list| {
            list.iter()
                .any(|global| global.interface == "zwlr_layer_shell_v1")
        })
    })
}

pub fn set_config(config: CursorConfig) {
    CONFIG_ENABLED.store(config.enabled, Ordering::Release);
    let _ = CONFIG_TEMPLATE.set(config);
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
    // If the owner has never started, it cannot hold a tombstone. Accept the
    // lifecycle transition without paying the compositor startup cost.
    if matches!(msg, OverlayMsg::Revive(_)) && tx().is_none() {
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
    map_overlay_msg(msg).is_some_and(|cmd| tx.try_send(cmd).is_ok())
}

fn map_overlay_msg(msg: &OverlayMsg) -> Option<WlOverlayCmd> {
    match msg {
        OverlayMsg::Remove(key) => Some(WlOverlayCmd::Remove(key.clone())),
        OverlayMsg::Cmd(kc) if matches!(&kc.cmd, OverlayCommand::ShowFocusRect(_)) => None,
        OverlayMsg::Cmd(kc) => Some(WlOverlayCmd::Cmd {
            key: kc.key.clone(),
            cmd: kc.cmd.clone(),
        }),
        OverlayMsg::Revive(key) => Some(WlOverlayCmd::Revive(key.clone())),
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
    /// Configured layer surfaces that have received at least one wl_buffer.
    /// This is intentionally separate from `painted_outputs`: every new or
    /// reconfigured surface needs a one-shot transparent commit to complete
    /// its layer-shell handshake, but stable empty surfaces must not trigger
    /// recurring full-output SHM redraws.
    initialized_outputs: HashSet<u32>,
    topology_dirty: bool,
    /// Keyed render cores mirror the X11 native overlay contract. Removing a
    /// named key records it in `ended`, so already-queued late commands cannot
    /// recreate a cursor after end_session.
    cores: HashMap<CursorKey, RenderStateCore>,
    template: CursorConfig,
    ended: HashSet<CursorKey>,
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

impl Drop for OverlayState {
    fn drop(&mut self) {
        for (_, (ptr, size, fd)) in std::mem::take(&mut self.pending_buffers) {
            super::cleanup_mmap(ptr, size, fd);
        }
    }
}

impl OverlayState {
    fn new(template: CursorConfig) -> Self {
        let mut cores = HashMap::new();
        // Match the X11 contract: the compatibility/default slot preserves the
        // launch-time cursor id, while lazily-created named slots override it.
        cores.insert("default".to_owned(), RenderStateCore::new(template.clone()));
        Self {
            compositor: None,
            shm: None,
            layer_shell: None,
            xdg_output_manager: None,
            outputs: HashMap::new(),
            painted_outputs: HashSet::new(),
            initialized_outputs: HashSet::new(),
            topology_dirty: false,
            cores,
            template,
            ended: HashSet::new(),
            pending_buffers: HashMap::new(),
        }
    }
}

fn render_core_for_key(template: &CursorConfig, key: &str) -> RenderStateCore {
    let mut config = template.clone();
    config.cursor_id = key.to_owned();
    RenderStateCore::new(config)
}

fn apply_keyed_command(
    cores: &mut HashMap<CursorKey, RenderStateCore>,
    template: &CursorConfig,
    ended: &HashSet<CursorKey>,
    key: CursorKey,
    cmd: OverlayCommand,
) -> bool {
    if ended.contains(&key) {
        tracing::debug!(key = %key, cmd = ?cmd, "wayland overlay: command dropped — key was ended");
        return false;
    }

    let core = cores
        .entry(key.clone())
        .or_insert_with(|| render_core_for_key(template, &key));
    // Seed from the off-screen sentinel near the first targeted action so a
    // spring animation begins on-screen. This mirrors the X11 renderer.
    let seed_target = match &cmd {
        OverlayCommand::MoveTo { x, y, .. }
        | OverlayCommand::SnapTo { x, y, .. }
        | OverlayCommand::ClickPulse { x, y } => Some((*x, *y)),
        _ => None,
    };
    if let Some((target_x, target_y)) = seed_target {
        if core.pos.0 < -50.0 {
            const SEED_OFFSET: f64 = 140.0;
            core.pos = (
                (target_x - SEED_OFFSET).max(2.0),
                (target_y - SEED_OFFSET).max(2.0),
            );
        }
    }

    let disabling = matches!(&cmd, OverlayCommand::SetEnabled(false));
    let dirty = core.apply_command_base(cmd, false, false);
    if disabling {
        quiesce_hidden(core);
    }
    dirty
}

fn remove_keyed_core(
    cores: &mut HashMap<CursorKey, RenderStateCore>,
    ended: &mut HashSet<CursorKey>,
    key: CursorKey,
) -> bool {
    if key == "default" {
        return false;
    }
    let removed = cores.remove(&key).is_some();
    ended.insert(key);
    removed
}

fn revive_key(ended: &mut HashSet<CursorKey>, key: CursorKey) {
    if key != "default" {
        ended.remove(&key);
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
        .filter(|layout| output_contains(layout, x, y))
        .min_by_key(|layout| layout.id)
        .map(|layout| SelectedOutput {
            id: layout.id,
            local_x: x - f64::from(layout.origin_x),
            local_y: y - f64::from(layout.origin_y),
        })
}

fn output_contains(layout: &OutputLayout, x: f64, y: f64) -> bool {
    x.is_finite()
        && y.is_finite()
        && x >= f64::from(layout.origin_x)
        && y >= f64::from(layout.origin_y)
        && x < f64::from(layout.origin_x) + f64::from(layout.width)
        && y < f64::from(layout.origin_y) + f64::from(layout.height)
}

fn frame_plan(
    layouts: &[OutputLayout],
    painted_outputs: &HashSet<u32>,
    initialized_outputs: &HashSet<u32>,
    cursor_positions: impl IntoIterator<Item = (f64, f64)>,
) -> (HashSet<u32>, Vec<FrameTarget>) {
    let selected: HashSet<u32> = cursor_positions
        .into_iter()
        .filter_map(|(x, y)| select_output(layouts, x, y).map(|output| output.id))
        .collect();
    let mut ids: Vec<u32> = painted_outputs.iter().copied().collect();
    ids.extend(
        layouts
            .iter()
            .map(|layout| layout.id)
            .filter(|id| !initialized_outputs.contains(id)),
    );
    for id in &selected {
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    let targets = ids.into_iter().map(|id| FrameTarget { id }).collect();
    (selected, targets)
}

fn visible_cores_for_output<'a>(
    cores: &'a HashMap<CursorKey, RenderStateCore>,
    layouts: &[OutputLayout],
    output_id: u32,
) -> Vec<(&'a CursorKey, &'a RenderStateCore)> {
    let mut visible_cores: Vec<_> = cores
        .iter()
        .filter(|(_, core)| {
            core.visible
                && core.pos.0 >= -100.0
                && core.idle_alpha >= 0.004
                && select_output(layouts, core.pos.0, core.pos.1)
                    .is_some_and(|selected| selected.id == output_id)
        })
        .collect();
    visible_cores.sort_by(|(left, _), (right, _)| left.cmp(right));
    visible_cores
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

    let template = CONFIG_TEMPLATE.get().cloned().unwrap_or_default();
    let mut state = OverlayState::new(template);
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

    // A layer-shell surface is not fully initialized until the first buffer is
    // attached after configure. Commit one transparent buffer to every empty
    // output now, before entering the demand-driven loop; a cursor already
    // selected on an output can share this same first frame.
    redraw(&mut state, &shm, &qh)?;
    queue.roundtrip(&mut state)?;

    // Demand-driven main loop. Stable cursors perform only a cheap Wayland
    // maintenance roundtrip once per second so output topology changes are
    // observed; full-display SHM work remains limited to visual changes.
    let mut last_tick = Instant::now();
    let mut frame_tick_needed = false;
    loop {
        let wait = next_wait(&state.cores, frame_tick_needed, state.topology_dirty);
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
                Ok(WlOverlayCmd::Cmd { key, cmd }) => {
                    dirty |= apply_keyed_command(
                        &mut state.cores,
                        &state.template,
                        &state.ended,
                        key,
                        cmd,
                    );
                }
                Ok(WlOverlayCmd::Remove(key)) => {
                    dirty |= remove_keyed_core(&mut state.cores, &mut state.ended, key);
                }
                Ok(WlOverlayCmd::Revive(key)) => {
                    revive_key(&mut state.ended, key);
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
                    tick_all_cores(&mut state.cores, elapsed.min(0.05));
                    dirty = true;
                }
                WlWait::Deadline(_) => {
                    tick_all_cores(&mut state.cores, elapsed);
                    dirty = true;
                }
                WlWait::Maintenance(_) => {
                    let before: HashMap<CursorKey, f64> = state
                        .cores
                        .iter()
                        .map(|(key, core)| (key.clone(), core.idle_alpha))
                        .collect();
                    tick_all_cores(&mut state.cores, elapsed);
                    dirty |= state.cores.iter().any(|(key, core)| {
                        before
                            .get(key)
                            .is_none_or(|alpha| *alpha != core.idle_alpha)
                            || needs_frame_tick(core)
                    });

                    let topology_was_dirty = state.topology_dirty;
                    state.topology_dirty = false;
                    queue.roundtrip(&mut state)?;
                    ensure_output_resources(&mut state, &qh);
                    dirty |= topology_was_dirty || state.topology_dirty;
                    state.topology_dirty = false;
                }
            }
        }
        let next_frame_tick_needed = any_core_needs_frame_tick(&state.cores);
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

fn next_wait(
    cores: &HashMap<CursorKey, RenderStateCore>,
    frame_tick_needed: bool,
    topology_dirty: bool,
) -> WlWait {
    if topology_dirty {
        return WlWait::Maintenance(Duration::ZERO);
    }
    if frame_tick_needed || any_core_needs_frame_tick(cores) {
        return WlWait::Frame;
    }
    match earliest_idle_fade_wait(cores) {
        Some(wait) if wait <= TOPOLOGY_MAINTENANCE_INTERVAL => WlWait::Deadline(wait),
        _ => WlWait::Maintenance(TOPOLOGY_MAINTENANCE_INTERVAL),
    }
}

fn any_core_needs_frame_tick(cores: &HashMap<CursorKey, RenderStateCore>) -> bool {
    cores.values().any(needs_frame_tick)
}

fn earliest_idle_fade_wait(cores: &HashMap<CursorKey, RenderStateCore>) -> Option<Duration> {
    cores.values().filter_map(idle_fade_wait).min()
}

fn tick_all_cores(cores: &mut HashMap<CursorKey, RenderStateCore>, dt: f64) {
    for core in cores.values_mut() {
        core.tick_motion(dt);
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

/// Composite all visible cursor cores into their selected output and attach a
/// transparent clearing frame to every previously painted output now empty.
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
/// Hidden, idle-faded, or off-screen cores paint nothing.
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
    let cursor_positions = state
        .cores
        .values()
        .filter(|core| core.visible && core.pos.0 >= -100.0 && core.idle_alpha >= 0.004)
        .map(|core| core.pos);
    let (selected, targets) = frame_plan(
        &layouts,
        &state.painted_outputs,
        &state.initialized_outputs,
        cursor_positions,
    );

    for target in targets {
        redraw_output(state, shm, qh, target, &layouts)?;
    }

    state.painted_outputs = selected;
    Ok(())
}

fn redraw_output(
    state: &mut OverlayState,
    shm: &WlShm,
    qh: &QueueHandle<OverlayState>,
    target: FrameTarget,
    layouts: &[OutputLayout],
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

    // A clearing target intentionally stays transparent. Each cursor selected
    // for this output subtracts its logical compositor origin from the global
    // position; this is the same origin contract used by the Windows virtual
    // desktop.
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

    // Reuses the same anon_shm pattern as the screencopy path in mod.rs. The
    // pixmap is allocated first so a tiny-skia OOM cannot strand this mmap/fd.
    let (fd, ptr) =
        super::anon_shm(size).map_err(|e| anyhow::anyhow!("overlay shm allocation failed: {e}"))?;

    // SAFETY: ptr came from mmap of `size` bytes and is transferred to
    // `pending_buffers` before this function returns successfully.
    let pixels: &mut [u8] = unsafe { std::slice::from_raw_parts_mut(ptr as *mut u8, size) };
    let mut painted_positions = Vec::new();
    {
        // HashMap iteration is intentionally normalized by key so overlapping
        // named cursors composite deterministically from frame to frame.
        for (_, core) in visible_cores_for_output(&state.cores, layouts, target.id) {
            cursor_overlay::paint_cursor(
                &mut pm,
                core,
                f64::from(layout.origin_x),
                f64::from(layout.origin_y),
                None,
                1.0,
            );
            painted_positions.push(core.pos);
        }
    }

    // CUA_OVERLAY_DEBUG=1 paints a 60x60 magenta square at the cursor's
    // current pos on top of the gradient arrow. Useful when validating
    // layer-shell visibility on a new compositor — the gradient arrow is
    // small at native scale and easy to miss in a screenshot, while the
    // magenta block is impossible to miss.
    if std::env::var_os("CUA_OVERLAY_DEBUG").is_some() {
        for &(cursor_x, cursor_y) in &painted_positions {
            let cx = (cursor_x - f64::from(layout.origin_x)) as i32;
            let cy = (cursor_y - f64::from(layout.origin_y)) as i32;
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
        "redraw output={} origin=({}, {}) w={w} h={h} stride={stride} buf_id={buffer_id} cursors={}",
        output_label(state, target.id),
        layout.origin_x,
        layout.origin_y,
        painted_positions.len(),
    ));
    surface.attach(Some(&buffer), 0, 0);
    // Damage both coordinate spaces. Some wlroots compositors otherwise leave
    // stale transparent frames behind while an animated cursor is moving.
    surface.damage(0, 0, w as i32, h as i32);
    surface.damage_buffer(0, 0, w as i32, h as i32);
    surface.commit();
    // Mark initialized only after the buffer attach + surface commit succeed.
    // An early return above leaves the output pending for the next plan.
    state.initialized_outputs.insert(target.id);
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
                state.initialized_outputs.remove(&name);
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
                    // Each configure starts a new layer-surface buffer cycle.
                    // Queue exactly one matching clear/paint frame even when
                    // the compositor repeats the same logical size.
                    state.initialized_outputs.remove(&data.id);
                    state.topology_dirty = true;
                    output.configured_size = Some(configured_size);
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.painted_outputs.remove(&data.id);
                state.initialized_outputs.remove(&data.id);
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

    fn initialized(layouts: &[OutputLayout]) -> HashSet<u32> {
        layouts.iter().map(|layout| layout.id).collect()
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
    fn overlapping_outputs_use_the_same_deterministic_selection_for_compositing() {
        let layouts = vec![
            OutputLayout {
                id: 8,
                origin_x: 0,
                origin_y: 0,
                width: 1920,
                height: 1080,
            },
            OutputLayout {
                id: 4,
                origin_x: 0,
                origin_y: 0,
                width: 1920,
                height: 1080,
            },
        ];
        let mut core = positioned_core();
        core.pos = (400.0, 300.0);
        let cores = HashMap::from([("session".to_owned(), core)]);

        assert_eq!(select_output(&layouts, 400.0, 300.0).unwrap().id, 4);
        assert_eq!(visible_cores_for_output(&cores, &layouts, 4).len(), 1);
        assert!(visible_cores_for_output(&cores, &layouts, 8).is_empty());
    }

    #[test]
    fn initial_configured_outputs_each_plan_one_transparent_frame() {
        let layouts = three_monitor_layout();
        let (selected, targets) = frame_plan(
            &layouts,
            &HashSet::new(),
            &HashSet::new(),
            std::iter::empty::<(f64, f64)>(),
        );

        assert!(selected.is_empty());
        assert_eq!(
            targets.iter().map(|target| target.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn initialized_stable_outputs_plan_no_idle_frame() {
        let layouts = three_monitor_layout();
        let (selected, targets) = frame_plan(
            &layouts,
            &HashSet::new(),
            &initialized(&layouts),
            std::iter::empty::<(f64, f64)>(),
        );

        assert!(selected.is_empty());
        assert!(targets.is_empty());
    }

    #[test]
    fn hotplugged_output_plans_one_initial_transparent_frame() {
        let layouts = three_monitor_layout();
        let already_initialized = HashSet::from([1, 2]);
        let (selected, targets) = frame_plan(
            &layouts,
            &HashSet::new(),
            &already_initialized,
            std::iter::empty::<(f64, f64)>(),
        );

        assert!(selected.is_empty());
        assert_eq!(targets, vec![FrameTarget { id: 3 }]);
    }

    #[test]
    fn selected_output_combines_initialization_and_cursor_paint_in_one_frame() {
        let layouts = three_monitor_layout();
        let already_initialized = HashSet::from([1, 3]);
        let (selected, targets) = frame_plan(
            &layouts,
            &HashSet::new(),
            &already_initialized,
            Some((2500.0, 100.0)),
        );

        assert_eq!(selected, HashSet::from([2]));
        assert_eq!(targets, vec![FrameTarget { id: 2 }]);
    }

    #[test]
    fn crossing_outputs_clears_the_old_surface_and_paints_the_new_one() {
        let layouts = three_monitor_layout();
        let painted = HashSet::from([1]);
        let (selected, targets) = frame_plan(
            &layouts,
            &painted,
            &initialized(&layouts),
            Some((2500.0, 100.0)),
        );

        assert_eq!(selected, HashSet::from([2]));
        assert_eq!(targets, vec![FrameTarget { id: 1 }, FrameTarget { id: 2 }]);
    }

    #[test]
    fn hide_or_session_removal_clears_every_previously_painted_output() {
        let layouts = three_monitor_layout();
        let painted = HashSet::from([1, 2, 3]);
        let (selected, targets) = frame_plan(
            &layouts,
            &painted,
            &initialized(&layouts),
            std::iter::empty::<(f64, f64)>(),
        );

        assert!(selected.is_empty());
        assert_eq!(targets.len(), 3);
        assert_eq!(
            targets.iter().map(|target| target.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn fresh_sentinel_overlay_uses_only_topology_maintenance() {
        let layouts = three_monitor_layout();
        let core = RenderStateCore::new(CursorConfig::default());
        let cores = HashMap::from([("default".to_owned(), core)]);
        assert_eq!(
            next_wait(&cores, false, false),
            WlWait::Maintenance(TOPOLOGY_MAINTENANCE_INTERVAL)
        );
        assert!(!any_core_needs_frame_tick(&cores));
        assert_eq!(
            next_wait(&cores, false, true),
            WlWait::Maintenance(Duration::ZERO)
        );
        assert!(frame_plan(
            &layouts,
            &HashSet::new(),
            &initialized(&layouts),
            std::iter::empty::<(f64, f64)>()
        )
        .1
        .is_empty());
    }

    #[test]
    fn stable_visible_overlay_sleeps_until_idle_fade_deadline() {
        let mut core = positioned_core();
        core.idle_secs = 0.25;
        let cores = HashMap::from([("cursor-a".to_owned(), core)]);
        assert_eq!(
            next_wait(&cores, false, false),
            WlWait::Deadline(Duration::from_millis(750))
        );
        assert!(!any_core_needs_frame_tick(&cores));
    }

    #[test]
    fn animation_and_fade_use_frame_cadence() {
        let mut core = positioned_core();
        core.click_t = Some(0.0);
        assert!(needs_frame_tick(&core));
        let mut cores = HashMap::from([("cursor-a".to_owned(), core)]);
        assert_eq!(next_wait(&cores, false, false), WlWait::Frame);

        let core = cores.get_mut("cursor-a").unwrap();
        core.click_t = None;
        core.idle_secs = 1.0;
        core.idle_alpha = 1.0;
        assert!(needs_frame_tick(core));
        assert_eq!(next_wait(&cores, false, false), WlWait::Frame);
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
        let cores = HashMap::from([("cursor-a".to_owned(), core)]);
        assert_eq!(
            next_wait(&cores, false, false),
            WlWait::Maintenance(TOPOLOGY_MAINTENANCE_INTERVAL)
        );
    }

    #[test]
    fn forward_mapping_preserves_named_cursor_keys() {
        let snap = message(OverlayCommand::SnapTo {
            x: 10.0,
            y: 20.0,
            heading_radians: None,
        });
        assert!(matches!(
            map_overlay_msg(&snap),
            Some(WlOverlayCmd::Cmd { key, .. }) if key == "test"
        ));
        assert!(matches!(
            map_overlay_msg(&OverlayMsg::Remove("session-a".to_owned())),
            Some(WlOverlayCmd::Remove(key)) if key == "session-a"
        ));
        assert!(matches!(
            map_overlay_msg(&OverlayMsg::Revive("session-a".to_owned())),
            Some(WlOverlayCmd::Revive(key)) if key == "session-a"
        ));
    }

    #[test]
    fn named_cursors_render_on_independent_outputs_and_removal_clears_only_one() {
        let template = CursorConfig::default();
        let mut cores = HashMap::new();
        let mut ended = HashSet::new();
        assert!(apply_keyed_command(
            &mut cores,
            &template,
            &ended,
            "session-a".to_owned(),
            OverlayCommand::SnapTo {
                x: 100.0,
                y: 100.0,
                heading_radians: None,
            },
        ));
        assert!(apply_keyed_command(
            &mut cores,
            &template,
            &ended,
            "session-b".to_owned(),
            OverlayCommand::SnapTo {
                x: 2500.0,
                y: 100.0,
                heading_radians: None,
            },
        ));
        assert_eq!(cores["session-a"].cfg.cursor_id, "session-a");
        assert_eq!(cores["session-b"].cfg.cursor_id, "session-b");

        let layouts = three_monitor_layout();
        assert_eq!(
            visible_cores_for_output(&cores, &layouts, 1)
                .into_iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            vec!["session-a"]
        );
        assert_eq!(
            visible_cores_for_output(&cores, &layouts, 2)
                .into_iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            vec!["session-b"]
        );
        let (painted, targets) = frame_plan(
            &layouts,
            &HashSet::new(),
            &initialized(&layouts),
            cores.values().map(|core| core.pos),
        );
        assert_eq!(painted, HashSet::from([1, 2]));
        assert_eq!(targets, vec![FrameTarget { id: 1 }, FrameTarget { id: 2 }]);

        assert!(remove_keyed_core(
            &mut cores,
            &mut ended,
            "session-a".to_owned()
        ));
        assert!(!cores.contains_key("session-a"));
        assert!(cores.contains_key("session-b"));
        assert!(visible_cores_for_output(&cores, &layouts, 1).is_empty());
        assert_eq!(
            visible_cores_for_output(&cores, &layouts, 2)
                .into_iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            vec!["session-b"]
        );
        let (selected, targets) = frame_plan(
            &layouts,
            &painted,
            &initialized(&layouts),
            cores.values().map(|core| core.pos),
        );
        assert_eq!(selected, HashSet::from([2]));
        assert_eq!(targets, vec![FrameTarget { id: 1 }, FrameTarget { id: 2 }]);

        // A queued command cannot resurrect an ended named session.
        assert!(!apply_keyed_command(
            &mut cores,
            &template,
            &ended,
            "session-a".to_owned(),
            OverlayCommand::SnapTo {
                x: 200.0,
                y: 200.0,
                heading_radians: None,
            },
        ));
        assert!(!cores.contains_key("session-a"));

        revive_key(&mut ended, "session-a".to_owned());
        assert!(!ended.contains("session-a"));
        assert!(apply_keyed_command(
            &mut cores,
            &template,
            &ended,
            "session-a".to_owned(),
            OverlayCommand::SnapTo {
                x: 200.0,
                y: 200.0,
                heading_radians: None,
            },
        ));
        assert!(cores.contains_key("session-a"));
    }

    #[test]
    fn named_cursors_schedule_animation_and_idle_deadlines_independently() {
        let mut idle = positioned_core();
        idle.idle_secs = 0.25;
        let mut animated = positioned_core();
        animated.motion.idle_hide_ms = 4_000.0;
        animated.click_t = Some(0.0);
        let mut cores =
            HashMap::from([("idle".to_owned(), idle), ("animated".to_owned(), animated)]);

        assert_eq!(next_wait(&cores, false, false), WlWait::Frame);
        cores.get_mut("animated").unwrap().click_t = None;
        assert_eq!(
            next_wait(&cores, false, false),
            WlWait::Deadline(Duration::from_millis(750))
        );
    }

    #[test]
    fn default_cursor_is_not_removed_or_marked_ended() {
        let mut cores = HashMap::from([(
            "default".to_owned(),
            RenderStateCore::new(CursorConfig::default()),
        )]);
        let mut ended = HashSet::new();
        assert!(!remove_keyed_core(
            &mut cores,
            &mut ended,
            "default".to_owned()
        ));
        assert!(cores.contains_key("default"));
        assert!(!ended.contains("default"));
    }

    #[test]
    fn maintenance_scheduler_wakes_immediately_on_command_arrival() {
        let (tx, rx) = bounded(1);
        tx.send(WlOverlayCmd::Remove("test".to_owned())).unwrap();
        assert!(matches!(
            wait_for_work(&rx, WlWait::Maintenance(Duration::from_secs(1))),
            WlWake::Command(WlOverlayCmd::Remove(key)) if key == "test"
        ));
    }

    #[test]
    fn disabled_config_refuses_forwarding_and_thread_startup() {
        CONFIG_ENABLED.store(false, Ordering::Release);
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
