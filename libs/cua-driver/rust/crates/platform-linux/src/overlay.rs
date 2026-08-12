//! Linux agent-cursor overlay — X11 RGBA override-redirect window.
//!
//! Architecture:
//! - Creates an override-redirect (non-reparented) X11 window with 32-bit ARGB visual
//!   from XComposite.  The window covers the full display area.
//! - A background thread renders cursor-local tiles at ~60 Hz while animation
//!   is active and uploads them with XPutImage + XShape clipping.
//! - XShape clips both input and visible pixels. On bare X11 the server never
//!   blends our alpha, so the tile is software-composited over a save-under
//!   copy of the real desktop backdrop and uploaded opaque; the visible shape
//!   stays the alpha≠0 runs, exactly as under a compositor. When the backdrop
//!   cannot be read the frame is deferred, and if that persists the shape falls
//!   back to quantizing the alpha mask for the session, which keeps the cursor
//!   visible at the cost of the translucent bloom.
//! - Software compositing reads the root under each tile. Our own painted
//!   pixels would come back with it, so every painted rect is recorded with
//!   both the desktop that was under it and the pixels we put on top; a later
//!   read of that rect is only treated as desktop once it stops matching what
//!   we uploaded. Whenever that record cannot be trusted — a RandR change, a
//!   compositing manager arriving or leaving — the overlay blanks itself for a
//!   short grace so the windows underneath repaint before it reads again.
//! - Known cost of compositing without a compositor: the alpha≠0 footprint is
//!   opaque on screen, so whatever ends up under a *resting* cursor cannot be
//!   observed (the server clips those pixels away from their owner) and stays
//!   as it was at the last paint until the cursor moves or fades. The cutoff
//!   path had the same limitation over the smaller alpha≥128 silhouette.
//! - Z-ordering: `XRaiseWindow` every 80ms to stay above normal windows.
//! - Wayland: when WAYLAND_DISPLAY is set but DISPLAY is also available (XWayland),
//!   the X11 path is used.  Pure Wayland support is a TODO.
//!
//! ## Cross-platform note (2026-05 dedup audit)
//!
//! Animation state + render pipeline live in `cursor_overlay::render_state`
//! (`RenderStateCore`, `tick_motion`, `apply_command_base`, `render_frame`).
//! What stays here is the X11 window plumbing: connection setup,
//! override-redirect visual, ShapeInput passthrough, and the XPutImage paint.

#[cfg(target_os = "linux")]
use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(all(test, target_os = "linux"))]
use cursor_overlay::CursorAction;
#[cfg(target_os = "linux")]
const X11_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How often the `_NET_WM_CM_S{n}` owner is re-sampled. A compositing manager
/// starting or stopping emits no event we subscribe to, and it decides whether
/// the server blends our alpha, so the sample cannot be startup-only.
#[cfg(target_os = "linux")]
const X11_COMPOSITOR_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[cfg(target_os = "linux")]
use cursor_overlay::ZOrderEnforcer;
use cursor_overlay::{
    CursorConfig, CursorKey, KeyedOverlayCommand, OverlayCommand, OverlayMsg, RenderStateCore,
};

// ── Global channel ────────────────────────────────────────────────────────

static CMD_TX: OnceLock<std::sync::mpsc::SyncSender<OverlayMsg>> = OnceLock::new();
static CMD_RX_CELL: Mutex<Option<std::sync::mpsc::Receiver<OverlayMsg>>> = Mutex::new(None);
static RENDER: Mutex<Option<RenderMap>> = Mutex::new(None);
static ARRIVAL_TX: Mutex<Option<HashMap<CursorKey, tokio::sync::oneshot::Sender<()>>>> =
    Mutex::new(None);
#[cfg(all(test, target_os = "linux"))]
static X11_RANDR_REPAIR_COUNT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

fn arrival_register(key: CursorKey, tx: tokio::sync::oneshot::Sender<()>) {
    let mut guard = ARRIVAL_TX.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(old_tx) = map.insert(key, tx) {
        let _ = old_tx.send(());
    }
}

fn arrival_fire(key: &CursorKey) {
    if let Ok(mut guard) = ARRIVAL_TX.lock() {
        if let Some(map) = guard.as_mut() {
            if let Some(tx) = map.remove(key) {
                let _ = tx.send(());
            }
        }
    }
}

fn arrival_cancel(key: &CursorKey) {
    if let Ok(mut guard) = ARRIVAL_TX.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(key);
        }
    }
}

fn release_all_arrivals() {
    if let Ok(mut guard) = ARRIVAL_TX.lock() {
        clear_arrivals(&mut guard);
    }
}

fn clear_arrivals(arrivals: &mut Option<HashMap<CursorKey, tokio::sync::oneshot::Sender<()>>>) {
    if let Some(map) = arrivals.as_mut() {
        map.clear();
    }
}

fn try_send_x11_message(
    sender: Option<&std::sync::mpsc::SyncSender<OverlayMsg>>,
    msg: OverlayMsg,
) -> bool {
    sender.is_some_and(|tx| tx.try_send(msg).is_ok())
}

#[cfg(target_os = "linux")]
struct X11OverlayThreadCleanup {
    receiver: Option<std::sync::mpsc::Receiver<OverlayMsg>>,
    disable_render_state: bool,
}

#[cfg(target_os = "linux")]
impl X11OverlayThreadCleanup {
    fn receiver(&self) -> &std::sync::mpsc::Receiver<OverlayMsg> {
        self.receiver
            .as_ref()
            .expect("X11 overlay receiver is available before teardown")
    }

    fn disconnect_receiver(&mut self) {
        drop(self.receiver.take());
    }

    fn finish_cleanup(&self) {
        if self.disable_render_state {
            if let Ok(mut guard) = RENDER.lock() {
                disable_render_map(&mut guard);
            }
        }
        release_all_arrivals();
    }

    /// Run a hook in the only teardown interval where registration can race:
    /// after channel disconnection but before renderer/waiter cleanup.
    fn teardown_with_after_disconnect(&mut self, after_disconnect: impl FnOnce()) {
        self.disconnect_receiver();
        after_disconnect();
        self.finish_cleanup();
    }
}

#[cfg(target_os = "linux")]
impl Drop for X11OverlayThreadCleanup {
    fn drop(&mut self) {
        // Disconnect first so an animator racing teardown cannot enqueue after
        // the waiter sweep. Registrations before release are swept below;
        // registrations after release observe a disconnected channel and cancel
        // themselves. On X11, also make future calls observe the renderer as
        // unavailable. A Wayland session may still use its native forwarding
        // path even when the optional XWayland owner thread cannot start.
        self.teardown_with_after_disconnect(|| {});
    }
}

#[cfg(target_os = "linux")]
fn disable_render_map(render: &mut Option<RenderMap>) {
    *render = None;
}

struct RenderMap {
    cursors: HashMap<CursorKey, RenderState>,
    scr_w: u32,
    scr_h: u32,
    template: CursorConfig,
    ended: HashSet<CursorKey>,
    last_active: Option<CursorKey>,
}

fn render_state_for_key(template: &CursorConfig, key: &str) -> RenderState {
    let mut config = template.clone();
    config.cursor_id = key.to_owned();
    RenderState::new(config)
}

fn apply_msg(map: &mut RenderMap, msg: OverlayMsg) -> Option<CursorKey> {
    match msg {
        OverlayMsg::Remove(key) => {
            if key != "default" {
                map.cursors.remove(&key);
                if let Ok(mut guard) = ARRIVAL_TX.lock() {
                    if let Some(arrivals) = guard.as_mut() {
                        arrivals.remove(&key);
                    }
                }
                if map.last_active.as_deref() == Some(key.as_str()) {
                    map.last_active = None;
                }
                map.ended.insert(key);
            }
            None
        }
        OverlayMsg::Revive(key) => {
            if key != "default" {
                map.ended.remove(&key);
            }
            None
        }
        OverlayMsg::Cmd(KeyedOverlayCommand { key, cmd }) => {
            if map.ended.contains(&key) {
                tracing::debug!(key = %key, cmd = ?cmd, "overlay: command dropped — key was ended");
                return None;
            }
            let template = map.template.clone();
            let k = key.clone();
            let rs = map
                .cursors
                .entry(key)
                .or_insert_with(|| render_state_for_key(&template, &k));
            rs.apply_command(cmd);
            Some(k)
        }
    }
}

pub fn init(cfg: CursorConfig) {
    static INITIALIZED: OnceLock<()> = OnceLock::new();
    INITIALIZED.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        CMD_TX
            .set(tx)
            .expect("cursor overlay sender is initialized exactly once");
        *CMD_RX_CELL.lock().unwrap() = Some(rx);
        *ARRIVAL_TX.lock().unwrap() = Some(HashMap::new());
        let mut cursors = HashMap::new();
        cursors.insert("default".to_owned(), RenderState::new(cfg.clone()));
        *RENDER.lock().unwrap() = Some(RenderMap {
            cursors,
            scr_w: 1920,
            scr_h: 1080,
            template: cfg,
            ended: HashSet::new(),
            last_active: None,
        });
    });
    cua_driver_core::cursor_events::install_cursor_event_sink(std::sync::Arc::new(
        |event: cua_driver_core::cursor_events::CursorEvent| {
            use cua_driver_core::cursor_events::{CursorEvent, CursorEventPhase};
            let (session, cmd) = match event {
                CursorEvent::SetSessionLabel { session, label } => {
                    (session, OverlayCommand::SetSessionLabel(label))
                }
                CursorEvent::Action {
                    session,
                    phase: CursorEventPhase::Begin,
                    semantics,
                } => (
                    session,
                    OverlayCommand::BeginAction {
                        action: semantics.action,
                        delivery: semantics.delivery,
                        target: semantics.target,
                    },
                ),
                CursorEvent::Action {
                    session,
                    phase: CursorEventPhase::End,
                    semantics,
                } => (session, OverlayCommand::EndAction(semantics.action)),
                CursorEvent::SelectTheme { session, selection } => (
                    session,
                    OverlayCommand::SetTheme {
                        theme_id: selection.theme_id,
                        reduced_motion: selection.reduced_motion,
                    },
                ),
            };
            send_command_for(session, cmd);
        },
    ));
}

pub fn send_command(cmd: OverlayCommand) {
    send_command_for("default".to_owned(), cmd);
}

pub fn send_command_for(key: CursorKey, cmd: OverlayCommand) {
    let _ = try_send_command_for(key, cmd);
}

/// Dispatch to every active Linux overlay backend. The result reports only
/// whether the X11 owner accepted the command and can fire `ARRIVAL_TX`; the
/// Wayland layer-shell path does not currently publish arrival notifications.
fn try_send_command_for(key: CursorKey, cmd: OverlayCommand) -> bool {
    if key.is_empty() {
        return false;
    }
    let msg = OverlayMsg::Cmd(KeyedOverlayCommand {
        key: key.clone(),
        cmd: cmd.clone(),
    });
    let x11_queued = try_send_x11_message(CMD_TX.get(), msg.clone());
    if !x11_queued {
        tracing::warn!(
            key = %key,
            sender_missing = CMD_TX.get().is_none(),
            "overlay: X11 channel rejected command (no sender or queue full)"
        );
    }
    // Also forward to the native-Wayland layer-shell overlay when Wayland
    // is opted in. The wayland overlay's `forward` is a no-op when its
    // owner thread isn't started yet (which is the normal X11-only case).
    #[cfg(target_os = "linux")]
    {
        if crate::wayland::is_wayland() {
            if crate::wayland::shell_helper::semantic_cursor_available() {
                crate::wayland::shell_helper::set_cursor_color(&cursor_overlay::session_fill_hex(
                    &key,
                ));
                // GNOME has no layer-shell. Drive only the final positioning
                // commands through the compositor helper; it performs its own
                // easing and avoids starting a worker that must fail.
                match &cmd {
                    cursor_overlay::OverlayCommand::ClickPulse { x, y } => {
                        crate::wayland::shell_helper::click_pulse(*x as i32, *y as i32);
                    }
                    cursor_overlay::OverlayCommand::MoveTo { x, y, .. } => {
                        crate::wayland::shell_helper::move_cursor(*x as i32, *y as i32);
                    }
                    cursor_overlay::OverlayCommand::SnapTo { x, y, .. } => {
                        crate::wayland::shell_helper::move_cursor(*x as i32, *y as i32);
                    }
                    cursor_overlay::OverlayCommand::BeginAction {
                        action,
                        delivery,
                        target,
                    } => {
                        crate::wayland::shell_helper::set_cursor_state(
                            action.as_str(),
                            delivery.as_ref().map_or("", |value| value.as_str()),
                            target.as_ref().map_or("", |value| value.as_str()),
                            true,
                        );
                    }
                    cursor_overlay::OverlayCommand::EndAction(action) => {
                        crate::wayland::shell_helper::set_cursor_state(
                            action.as_str(),
                            "",
                            "",
                            false,
                        );
                    }
                    cursor_overlay::OverlayCommand::SetSessionLabel(label) => {
                        crate::wayland::shell_helper::set_session_label(
                            cursor_overlay::sanitize_session_label(label)
                                .as_deref()
                                .unwrap_or(""),
                        );
                    }
                    cursor_overlay::OverlayCommand::SetEnabled(false) => {
                        crate::wayland::shell_helper::hide_cursor();
                    }
                    _ => {}
                }
            } else if !crate::wayland::shell_helper::available() {
                let _ = crate::wayland::overlay::forward(&msg);
            }
        }
    }
    x11_queued
}

pub fn is_enabled() -> bool {
    is_enabled_for("default")
}

pub fn is_enabled_for(key: &str) -> bool {
    RENDER
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref().and_then(|m| {
                m.cursors
                    .get(key)
                    .or_else(|| m.cursors.get("default"))
                    .map(|rs| rs.core.visible)
            })
        })
        .unwrap_or(false)
}

/// Truthful render acknowledgement for lifecycle inspection. This checks the
/// exact session key and never inherits the seeded default cursor.
pub fn is_visible_for_session(key: &str) -> bool {
    RENDER
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .and_then(|map| map.cursors.get(key))
                .map(|rs| {
                    rs.core.cfg.enabled
                        && rs.core.visible
                        && rs.core.idle_alpha >= 0.004
                        && rs.core.pos.0 >= -100.0
                })
        })
        .unwrap_or(false)
}

pub fn current_position() -> (f64, f64) {
    current_position_for("default")
}

pub fn current_position_for(key: &str) -> (f64, f64) {
    RENDER
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref()
                .and_then(|m| m.cursors.get(key))
                .map(|rs| rs.core.pos)
        })
        .unwrap_or((-200.0, -200.0))
}

pub fn current_motion_for(key: &str) -> cursor_overlay::MotionConfig {
    RENDER
        .lock()
        .ok()
        .and_then(|guard| {
            guard.as_ref().and_then(|map| {
                map.cursors
                    .get(key)
                    .or_else(|| map.cursors.get("default"))
                    .map(|state| state.core.motion.clone())
            })
        })
        .unwrap_or_default()
}

pub fn current_theme_state_for(
    key: &str,
) -> Option<(
    String,
    String,
    String,
    Option<String>,
    cursor_overlay::CursorVisualState,
)> {
    let guard = RENDER.lock().ok()?;
    let map = guard.as_ref()?;
    let state = map
        .cursors
        .get(key)
        .or_else(|| map.cursors.get("default"))?;
    let (id, version, profile, fallback) = state.core.active_theme_metadata();
    Some((id, version, profile, fallback, state.core.visual.clone()))
}

fn seed_start_if_sentinel(key: &CursorKey, target_x: f64, target_y: f64) -> bool {
    const SEED_OFFSET: f64 = 140.0;
    let mut guard = RENDER.lock().unwrap();
    let Some(map) = guard.as_mut() else {
        return false;
    };
    if map.ended.contains(key) {
        return false;
    }
    let template = map.template.clone();
    let k = key.clone();
    let rs = map
        .cursors
        .entry(key.clone())
        .or_insert_with(|| render_state_for_key(&template, &k));
    if !(rs.core.cfg.enabled && rs.core.pos.0 < -50.0) {
        return false;
    }
    let max_x = map.scr_w.max(2) as f64 - 2.0;
    let max_y = map.scr_h.max(2) as f64 - 2.0;
    let mut sx = (target_x - SEED_OFFSET).clamp(2.0, max_x);
    let mut sy = (target_y - SEED_OFFSET).clamp(2.0, max_y);
    if (sx - target_x).abs() < 8.0 && (sy - target_y).abs() < 8.0 {
        sx = (target_x + SEED_OFFSET).clamp(2.0, max_x);
        sy = (target_y + SEED_OFFSET).clamp(2.0, max_y);
    }
    rs.core.pos = (sx, sy);
    true
}

pub async fn animate_cursor_to(x: f64, y: f64) {
    animate_cursor_to_for("default".to_owned(), x, y).await;
}

pub async fn animate_cursor_to_for(key: CursorKey, x: f64, y: f64) {
    if key.is_empty() {
        return;
    }
    seed_start_if_sentinel(&key, x, y);
    let should_animate = {
        let guard = RENDER.lock().unwrap();
        match guard.as_ref().and_then(|m| m.cursors.get(&key)) {
            Some(rs) if rs.core.cfg.enabled && rs.core.visible && rs.core.pos.0 > -50.0 => true,
            _ => false,
        }
    };
    if !should_animate {
        return;
    }

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    arrival_register(key.clone(), tx);

    if !try_send_command_for(
        key.clone(),
        OverlayCommand::MoveTo {
            x,
            y,
            end_heading_radians: std::f64::consts::FRAC_PI_4,
        },
    ) {
        // A full or disconnected channel cannot ever produce an arrival. Drop
        // the registered sender now so this async operation cannot hang.
        arrival_cancel(&key);
        return;
    }

    let _ = rx.await;
}

pub fn remove_cursor(key: CursorKey) {
    if key.is_empty() {
        return;
    }
    let msg = OverlayMsg::Remove(key);
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(msg.clone());
    }
    #[cfg(target_os = "linux")]
    if crate::wayland::is_wayland() && !crate::wayland::shell_helper::available() {
        let _ = crate::wayland::overlay::forward(&msg);
    }
}

/// Clear the X11 render-side tombstone after a successful explicit session
/// revival. Wayland has no keyed tombstone, so forwarding this lifecycle
/// signal there is an accepted no-op.
pub fn revive_cursor(key: CursorKey) {
    if key.is_empty() {
        return;
    }
    let msg = OverlayMsg::Revive(key);
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(msg.clone());
    }
    #[cfg(target_os = "linux")]
    if crate::wayland::is_wayland() && !crate::wayland::shell_helper::available() {
        let _ = crate::wayland::overlay::forward(&msg);
    }
}

/// Spawn the overlay on a dedicated thread.  Non-blocking.
pub fn run_on_thread() {
    let rx = match CMD_RX_CELL.lock().unwrap().take() {
        Some(r) => r,
        None => return,
    };

    let cfg = {
        let guard = RENDER.lock().unwrap();
        match &*guard {
            Some(map) => map.template.clone(),
            None => return,
        }
    };

    if !cfg.enabled {
        return;
    }

    // Wayland layer-shell overlay is started LAZILY on the first
    // send_command_for() that targets a Wayland session (see the
    // wayland::overlay::forward() path). Starting it eagerly here added
    // ~100-300ms to cua-driver mcp startup — enough to push the CI
    // `cursor-click-gif` test (20s budget for the full launch_app →
    // click → type sequence) over its limit, intermittently. The
    // forward() path's own ensure_started() OnceLock guarantees the
    // thread spins up on demand without losing any commands (the
    // first send_command_for that triggers it spawns the thread,
    // future commands reuse it).

    std::thread::Builder::new()
        .name("cua-overlay-x11".into())
        .spawn(move || {
            run_overlay_thread(cfg, rx);
        })
        .expect("spawn overlay thread");
}

// ── Animation state ───────────────────────────────────────────────────────
//
// The platform-agnostic fields + tick + apply_command + render pipeline live
// in `cursor_overlay::render_state` (2026-05 dedup audit). What stays here
// is the X11-specific screen dimensions.

struct RenderState {
    core: RenderStateCore,
}

impl RenderState {
    fn new(cfg: CursorConfig) -> Self {
        RenderState {
            core: RenderStateCore::new(cfg),
        }
    }

    fn tick(&mut self, dt: f64) -> bool {
        self.core.tick_motion(dt)
    }

    fn apply_command(&mut self, cmd: OverlayCommand) {
        // Linux uses the non-sentinel-snap behaviour for both MoveTo and
        // ClickPulse: every command updates `self.pos` unconditionally.
        // Custom-shape / gradient / focus-rect commands are not rendered on
        // Linux at present; `apply_command_base` consumes SetShape +
        // SetGradient and returns false for ShowFocusRect — both cases drop
        // the visual update silently so callers don't see an error.
        let _ = self.core.apply_command_base(cmd, false, false);
    }

    /// True while the render loop must wake at frame cadence because the next
    /// tick can change pixels. A brand-new sentinel cursor is deliberately
    /// quiescent, so an idle MCP server can park on bounded maintenance waits
    /// instead of rebuilding and repainting X11 cursor tiles at 60 fps.
    #[cfg(target_os = "linux")]
    fn needs_frame_tick(&self) -> bool {
        let fade_start = self.core.motion.idle_hide_ms / 1000.0;
        self.core.path.is_some()
            || self.core.spring.is_some()
            || self.core.click_t.is_some()
            || self.core.session_badge_needs_frame_tick()
            // The resting float bob (`shared_float_motion`) is part of the
            // cursor's visual identity, not a transient animation: it runs
            // whenever the cursor is on screen and reduced motion is off, so
            // a settled cursor must keep receiving frames or the bob freezes
            // mid-swing on Linux while macOS keeps levitating. The term dies
            // with `idle_alpha` once the idle fade completes, returning the
            // parked-overlay fast path to the fully hidden cursor.
            || (self.core.visible
                && self.core.pos.0 >= -100.0
                && self.core.idle_alpha >= 0.004
                && self.core.visual.reduced_motion != cursor_overlay::ReducedMotion::On)
            || (self.core.motion.idle_hide_ms > 0.0
                && self.core.visible
                && self.core.pos.0 >= -100.0
                && self.core.idle_secs >= fade_start
                && self.core.idle_alpha >= 0.004)
    }
}

#[cfg(target_os = "linux")]
fn render_map_needs_frame_tick(map: &RenderMap) -> bool {
    map.cursors.values().any(RenderState::needs_frame_tick)
}

#[cfg(target_os = "linux")]
fn render_map_needs_z_order_tick(map: &RenderMap) -> bool {
    map.cursors
        .values()
        .any(|rs| rs.core.visible && rs.core.idle_alpha >= 0.004 && rs.core.pos.0 >= -100.0)
}

#[cfg(target_os = "linux")]
fn tick_render_map(map: &mut RenderMap, dt: f64) -> Vec<CursorKey> {
    let mut arrived = Vec::new();
    for (key, rs) in map.cursors.iter_mut() {
        if rs.tick(dt) {
            arrived.push(key.clone());
        }
    }
    arrived
}

#[cfg(target_os = "linux")]
fn apply_messages_after_wake(
    map: &mut RenderMap,
    first_msg: Option<OverlayMsg>,
    rx: &std::sync::mpsc::Receiver<OverlayMsg>,
    parked_elapsed: Option<f64>,
) -> (Vec<CursorKey>, bool) {
    // For a parked wake, advance pre-existing quiescent state before commands
    // mutate it. This preserves each cursor's independent idle clock while
    // ensuring newly commanded animations render their initial frame at dt=0.
    let arrived = parked_elapsed
        .map(|dt| tick_render_map(map, dt))
        .unwrap_or_default();
    let mut had_msg = false;
    if let Some(msg) = first_msg {
        had_msg = true;
        if let Some(key) = apply_msg(map, msg) {
            map.last_active = Some(key);
        }
    }
    while let Ok(msg) = rx.try_recv() {
        had_msg = true;
        if let Some(key) = apply_msg(map, msg) {
            map.last_active = Some(key);
        }
    }
    (arrived, had_msg)
}

#[cfg(target_os = "linux")]
fn process_render_wake(
    map: &mut RenderMap,
    first_msg: Option<OverlayMsg>,
    rx: &std::sync::mpsc::Receiver<OverlayMsg>,
    elapsed_dt: f64,
    maintenance_timeout: bool,
    frame_tick_needed: bool,
) -> (Vec<CursorKey>, bool) {
    // Command receives and maintenance timeouts both wake a parked loop. Tick
    // the old state before draining the channel so even a command that arrives
    // just after recv_timeout reports Timeout starts at dt=0. Active-frame
    // wakes retain their existing apply-then-tick ordering; pre-ticking those
    // could fire an old path's arrival waiter after a replacement is registered.
    let parked_wake = first_msg.is_some() || maintenance_timeout;
    let (mut arrived, had_msg) =
        apply_messages_after_wake(map, first_msg, rx, parked_wake.then_some(elapsed_dt));
    if !parked_wake && (frame_tick_needed || had_msg) {
        arrived.extend(tick_render_map(map, elapsed_dt.min(0.05)));
    }
    (arrived, had_msg)
}

#[cfg(target_os = "linux")]
fn render_map_idle_wait_interval(map: &RenderMap) -> Option<Duration> {
    map.cursors
        .values()
        .filter_map(|rs| {
            let core = &rs.core;
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
        })
        .min()
}

#[cfg(target_os = "linux")]
fn next_maintenance_deadline(
    last_tick: Instant,
    last_z_order_tick: Instant,
    z_order_interval: Duration,
    z_order_tick_needed: bool,
    idle_wait_interval: Option<Duration>,
    x11_event_poll_interval: Duration,
) -> Instant {
    let mut deadline = last_tick + x11_event_poll_interval;
    if z_order_tick_needed {
        deadline = deadline.min(last_z_order_tick + z_order_interval);
    }
    if let Some(interval) = idle_wait_interval {
        deadline = deadline.min(last_tick + interval);
    }
    deadline
}

#[cfg(target_os = "linux")]
fn z_order_reassertion_needed(
    had_msg: bool,
    screen_changed: bool,
    pin_changed: bool,
    periodic_tick_needed: bool,
    periodic_tick_due: bool,
) -> bool {
    had_msg || screen_changed || pin_changed || (periodic_tick_needed && periodic_tick_due)
}

#[cfg(target_os = "linux")]
enum OverlayWake {
    Frame,
    Message(OverlayMsg),
    MaintenanceTimeout,
    Disconnected,
}

#[cfg(target_os = "linux")]
fn wait_for_overlay_work(
    rx: &std::sync::mpsc::Receiver<OverlayMsg>,
    frame_tick_needed: bool,
    maintenance_deadline: Instant,
) -> OverlayWake {
    if frame_tick_needed {
        return OverlayWake::Frame;
    }

    let timeout = maintenance_deadline.saturating_duration_since(Instant::now());
    match rx.recv_timeout(timeout) {
        Ok(msg) => OverlayWake::Message(msg),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => OverlayWake::MaintenanceTimeout,
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => OverlayWake::Disconnected,
    }
}

#[cfg(target_os = "linux")]
fn recoverable_x11_z_order_error(error: &x11rb::x11_utils::X11Error, overlay_win: u32) -> bool {
    // BadWindow: the sibling died between the liveness probe and the server
    // processing the restack. BadMatch: the sibling is not a sibling — a
    // reparenting WM moved the target under a frame window (or reparented it
    // between our ancestor resolution and the restack), so it no longer
    // shares the overlay's parent. Both mean only "this z-order nudge did
    // not land"; the next eligible reassertion retries with fresh state.
    matches!(
        error.error_kind,
        x11rb::protocol::ErrorKind::Window | x11rb::protocol::ErrorKind::Match
    ) && error.major_opcode == x11rb::protocol::xproto::CONFIGURE_WINDOW_REQUEST
        && error.extension_name.is_none()
        && error.bad_value != overlay_win
}

#[cfg(target_os = "linux")]
/// Return `Ok(true)` for display changes, `Ok(false)` for unrelated or safely
/// recoverable events, and `Err` for protocol failures that disable the overlay.
fn classify_x11_overlay_event(
    event: &x11rb::protocol::Event,
    overlay_win: u32,
) -> anyhow::Result<bool> {
    match event {
        // The pinned target can disappear (BadWindow) or be reparented under a
        // WM frame (BadMatch) after the synchronous probe in
        // X11ZOrderEnforcer::reassert but before its unchecked ConfigureWindow
        // request reaches the server; x11rb then delivers the error here after
        // the VoidCookie is dropped. This owner connection's only other
        // ConfigureWindow path is checked synchronously, so a non-overlay bad
        // value is the stale sibling and is safe to retry with fresh state on
        // the next eligible z-order reassertion.
        x11rb::protocol::Event::Error(error)
            if recoverable_x11_z_order_error(error, overlay_win) =>
        {
            tracing::debug!(
                stale_target = error.bad_value,
                "X11 overlay z-order sibling went stale during reassertion"
            );
            Ok(false)
        }
        x11rb::protocol::Event::Error(error) => {
            anyhow::bail!("X11 server rejected an overlay request: {error:?}")
        }
        x11rb::protocol::Event::RandrScreenChangeNotify(_)
        | x11rb::protocol::Event::RandrNotify(_) => Ok(true),
        _ => Ok(false),
    }
}

#[cfg(target_os = "linux")]
fn update_render_map_geometry(map: &mut RenderMap, width: u16, height: u16) {
    map.scr_w = u32::from(width);
    map.scr_h = u32::from(height);
}

#[cfg(target_os = "linux")]
fn drain_x11_overlay_events(
    conn: &impl x11rb::connection::Connection,
    overlay_win: u32,
) -> anyhow::Result<bool> {
    let mut display_changed = false;
    while let Some(event) = conn.poll_for_event()? {
        display_changed |= classify_x11_overlay_event(&event, overlay_win)?;
    }
    Ok(display_changed)
}

#[cfg(target_os = "linux")]
fn subscribe_x11_display_changes(
    conn: &impl x11rb::connection::Connection,
    root: u32,
) -> anyhow::Result<()> {
    use x11rb::protocol::randr::{ConnectionExt as RandrConnectionExt, NotifyMask};

    let notify_mask =
        NotifyMask::SCREEN_CHANGE | NotifyMask::CRTC_CHANGE | NotifyMask::OUTPUT_CHANGE;
    conn.randr_select_input(root, notify_mask)?.check()?;
    conn.flush()?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn current_x11_root_geometry(
    conn: &impl x11rb::connection::Connection,
    root: u32,
) -> anyhow::Result<(u16, u16)> {
    use x11rb::protocol::xproto::ConnectionExt as XprotoConnectionExt;

    let geometry = conn.get_geometry(root)?.reply()?;
    anyhow::ensure!(
        geometry.width > 0 && geometry.height > 0,
        "X11 root reported invalid geometry {}x{}",
        geometry.width,
        geometry.height
    );
    Ok((geometry.width, geometry.height))
}

#[cfg(target_os = "linux")]
fn prepare_x11_overlay_geometry(
    conn: &impl x11rb::connection::Connection,
    win: u32,
    width: u16,
    height: u16,
) -> anyhow::Result<()> {
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::{
        ClipOrdering, ConfigureWindowAux, ConnectionExt as XprotoConnectionExt,
    };

    // Hide the backing window before resizing it. If the server reset or
    // invalidated an old bounding shape during RandR reconfiguration, this
    // prevents a zero-filled full-root frame from becoming visible between the
    // ConfigureWindow request and the next cursor-local paint.
    conn.shape_rectangles(
        SO::SET,
        SK::BOUNDING,
        ClipOrdering::UNSORTED,
        win,
        0,
        0,
        &[],
    )?
    .check()?;
    clear_x11_overlay_input_shape(conn, win)?;
    conn.configure_window(
        win,
        &ConfigureWindowAux::new()
            .x(0)
            .y(0)
            .width(u32::from(width))
            .height(u32::from(height)),
    )?
    .check()?;
    conn.flush()?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn clear_x11_overlay_input_shape(
    conn: &impl x11rb::connection::Connection,
    win: u32,
) -> anyhow::Result<()> {
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::ClipOrdering;

    conn.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, win, 0, 0, &[])?
        .check()?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn map_x11_overlay_with_empty_input(
    conn: &impl x11rb::connection::Connection,
    win: u32,
) -> anyhow::Result<()> {
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::{ClipOrdering, ConnectionExt as XprotoConnectionExt};

    // Check both safety-critical shapes before mapping. If either request is
    // rejected, the full-root overlay must remain unmapped rather than falling
    // back to the server's default full-window input region.
    clear_x11_overlay_input_shape(conn, win)?;
    conn.shape_rectangles(
        SO::SET,
        SK::BOUNDING,
        ClipOrdering::UNSORTED,
        win,
        0,
        0,
        &[],
    )?
    .check()?;
    conn.map_window(win)?.check()?;
    conn.flush()?;
    Ok(())
}

// ── X11 thread ────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn run_overlay_thread(cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayMsg>) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as XprotoConnectionExt;
    use x11rb::protocol::xproto::{
        AtomEnum, ColormapAlloc, CreateGCAux, CreateWindowAux, EventMask, PropMode, WindowClass,
    };
    use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

    let cleanup = X11OverlayThreadCleanup {
        receiver: Some(rx),
        disable_render_state: !crate::wayland::is_wayland(),
    };
    let rx = cleanup.receiver();

    // Connect to X11.
    let (conn, screen_num) = match x11rb::connect(None) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("X11 overlay: cannot connect to display: {e}");
            return;
        }
    };

    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    // RandR events are the authoritative signal that the root geometry and
    // output layout may have changed. Running the full-root overlay without a
    // repair signal is unsafe: a stale/reset bounding shape can expose the
    // zero-filled backing window. Fail closed if subscription is unavailable.
    if let Err(e) = subscribe_x11_display_changes(&conn, root) {
        tracing::warn!("X11 overlay: cannot subscribe to RandR display changes: {e}");
        return;
    }
    // The setup screen is a connection-time snapshot. Query after subscribing
    // so a display change racing startup is either reflected here or queued as
    // a RandR event for the loop below.
    let (scr_w, scr_h) = match current_x11_root_geometry(&conn, root) {
        Ok((width, height)) => (u32::from(width), u32::from(height)),
        Err(e) => {
            tracing::warn!("X11 overlay: cannot query initial root geometry: {e}");
            return;
        }
    };
    let mut compositor_present = x11_compositor_present(&conn, screen_num);
    tracing::debug!(compositor_present, "X11 overlay compositor state");

    // Update render state with screen size.
    {
        let mut guard = RENDER.lock().unwrap();
        if let Some(map) = guard.as_mut() {
            map.scr_w = scr_w;
            map.scr_h = scr_h;
        }
    }

    // Find 32-bit ARGB visual for compositing.
    // Falls back to the default visual if XComposite 32-bit isn't available.
    let (visual_id, depth, colormap) = find_argb_visual(&conn, screen).unwrap_or((
        screen.root_visual,
        screen.root_depth,
        screen.default_colormap,
    ));

    // Create a matching colormap if we got a non-default visual.
    let colormap = if visual_id != screen.root_visual {
        let cm = conn.generate_id().unwrap();
        conn.create_colormap(ColormapAlloc::NONE, cm, root, visual_id)
            .ok();
        cm
    } else {
        colormap
    };

    // An ARGB visual is only worth having when a compositing manager will
    // blend it; the compositor-less path renders pre-blended opaque pixels
    // behind a shape mask and needs no alpha channel. It is also actively
    // hazardous on servers that accept an ARGB32 window but cannot show it —
    // xorgxrdp repaints shaped ARGB regions as solid black whenever its
    // refresh re-renders them, and answers root reads inside them with black,
    // which both blanks the cursor on screen and poisons every save-under
    // comparison into compositing the shadow over black. Those servers are
    // exactly the compositor-less ones, so the rule is simple: no compositor
    // at startup, no ARGB. `software_only` pins the software-composited path
    // for the session — a compositing manager appearing later blends the
    // opaque shaped window correctly, whereas switching paths would hand this
    // 24-bit window premultiplied translucent pixels it cannot represent.
    let (visual_id, depth, colormap, software_only) =
        if compositor_present || visual_id == screen.root_visual {
            (visual_id, depth, colormap, false)
        } else {
            conn.free_colormap(colormap).ok();
            tracing::debug!(
                "X11 overlay: no compositor at startup; using the root visual \
                 with software compositing"
            );
            (
                screen.root_visual,
                screen.root_depth,
                screen.default_colormap,
                true,
            )
        };
    // Belt and braces for servers whose root reads are unreliable even for
    // the visual chosen above: `resolve_backdrop` stops treating a readback
    // mismatch as proof of a repaint.
    let readback_untrusted = !x11_visual_readback_reliable(
        &conn,
        root,
        visual_id,
        depth,
        colormap,
        scr_w as u16,
        scr_h as u16,
    )
    .unwrap_or(false);

    // Create the overlay window.
    let win = conn.generate_id().unwrap();
    let win_aux = CreateWindowAux::new()
        .background_pixel(0)
        .border_pixel(0)
        .colormap(colormap)
        // Override-redirect = no window manager decoration, no focus.
        .override_redirect(1u32)
        // Input passthrough: do not receive button/key events.
        .event_mask(EventMask::NO_EVENT);

    conn.create_window(
        depth,
        win,
        root,
        0,
        0,
        scr_w as u16,
        scr_h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        visual_id,
        &win_aux,
    )
    .ok();

    // Set window title (identifies our overlay, matches Windows convention).
    // `Cua.` namespace mirrors the Windows class-name + install-path
    // convention; was `TropeCUA.` (leaked codename from an early C# ref).
    let title = format!("Cua.AgentCursorOverlay.{}", cfg.cursor_id);
    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )
    .ok();

    // Start with empty input AND bounding regions before mapping. The empty
    // input region makes the window click-through. The empty bounding region
    // prevents a zero-filled full-screen window from appearing opaque black on
    // bare/non-composited X servers before the first cursor command paints a
    // real visible shape. ShapeMask with a None pixmap would reset either
    // region to the full window; an empty rectangle list expresses emptiness.
    if let Err(e) = map_x11_overlay_with_empty_input(&conn, win) {
        tracing::warn!(
            "X11 overlay: cannot establish click-through input shape; overlay remains unmapped: {e}"
        );
        return;
    }

    // One GC is sufficient for the lifetime of the overlay window. Recreating
    // and freeing it every 16 ms adds two avoidable X11 requests to the hot
    // path and compounds server pressure during sustained cursor movement.
    let gc_id = match conn.generate_id() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!("X11 overlay: cannot allocate graphics context id: {e}");
            return;
        }
    };
    if let Err(e) = conn.create_gc(gc_id, win, &CreateGCAux::new()) {
        tracing::warn!("X11 overlay: cannot create graphics context: {e}");
        return;
    }
    let paint_target = X11PaintTarget {
        win,
        root,
        depth,
        gc_id,
    };

    // Render at ~60 Hz only while pixels can change. Quiescent cursors use
    // bounded channel waits so X11/RandR events are serviced without repainting;
    // the final frame remains intact between maintenance wakes.
    let frame_dur = Duration::from_millis(16);
    let z_order_interval = Duration::from_millis(80);
    let mut last_tick = Instant::now();
    let mut last_ztick = Instant::now();
    let mut frame_tick_needed = false;
    let mut maintenance_deadline = last_tick + X11_EVENT_POLL_INTERVAL;
    let mut last_pinned: Option<u64> = None;
    let mut last_compositor_poll = last_tick;
    // Constructed after the geometry query and the window map, so the cache can
    // never be primed against a placeholder geometry. This window has painted
    // nothing yet and its bounding shape is still empty, so its first root read
    // sees no pixels of ours. (A previous overlay instance torn down moments
    // earlier can still be on screen; that resolves itself as soon as the
    // cursor vacates the rect and its owner repaints.)
    let mut backdrop = X11BackdropCache::default();
    // Startup probe result; a property of the server, not of the compositor,
    // so it is never re-sampled when a compositing manager comes or goes.
    backdrop.readback_untrusted = readback_untrusted;
    if readback_untrusted {
        tracing::warn!(
            "X11 overlay: root reads cannot see this window's own pixels; \
             save-unders will be served without readback confirmation"
        );
    }
    // Arrivals resolve callers waiting for the destination frame to be visible,
    // so they are held back across a deferred paint instead of firing early.
    let mut pending_arrivals: Vec<CursorKey> = Vec::new();
    let z_enforcer = X11ZOrderEnforcer {
        conn: &conn,
        win,
        root,
    };

    loop {
        // Idle fast path: no Pixmap allocation, RGBA→BGRA copy, XShape update,
        // or XPutImage until pixels can change. The 50 ms maintenance bound
        // services X11/RandR events; a resting visible cursor also reasserts
        // z-order at most every 80 ms. Neither maintenance path authorizes paint.
        let (first_msg, maintenance_timeout) =
            match wait_for_overlay_work(rx, frame_tick_needed, maintenance_deadline) {
                OverlayWake::Frame => (None, false),
                OverlayWake::Message(msg) => (Some(msg), false),
                OverlayWake::MaintenanceTimeout => (None, true),
                OverlayWake::Disconnected => break,
            };

        // X11 events do not wake std::mpsc, so quiescent overlays use the
        // bounded maintenance timeout above to service this queue. Detect
        // RandR changes before touching render state; unrelated X events remain
        // cheap and do not authorize a repaint.
        let screen_changed = match drain_x11_overlay_events(&conn, win) {
            Ok(changed) => changed,
            Err(e) => {
                tracing::warn!("X11 overlay event drain failed; disabling overlay: {e}");
                break;
            }
        };
        let screen_geometry = if screen_changed {
            let geometry = match current_x11_root_geometry(&conn, root) {
                Ok(geometry) => geometry,
                Err(e) => {
                    tracing::warn!(
                        "X11 overlay root geometry refresh failed; disabling overlay: {e}"
                    );
                    break;
                }
            };
            if let Err(e) = prepare_x11_overlay_geometry(&conn, win, geometry.0, geometry.1) {
                tracing::warn!("X11 overlay geometry repair failed; disabling overlay: {e}");
                break;
            }
            // Every saved backdrop describes the pre-resize screen. The repair
            // above emptied our bounding shape, but that only queues Expose:
            // the framebuffer still holds our last frame until the windows
            // underneath repaint it, so stay blank for the resync grace instead
            // of reading our own pixels back as desktop.
            backdrop.resync(Instant::now());
            #[cfg(test)]
            X11_RANDR_REPAIR_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            compositor_present = x11_compositor_present(&conn, screen_num);
            last_compositor_poll = Instant::now();
            tracing::debug!(
                width = geometry.0,
                height = geometry.1,
                compositor_present,
                "X11 overlay repaired after RandR display change"
            );
            Some(geometry)
        } else {
            None
        };

        // A compositing manager can start or stop without any RandR event, and
        // it decides which pixel policy is correct: with one present the server
        // blends our alpha and a root read no longer sees the windows below us.
        // Poll the selection owner rather than let a stale sample pick the path.
        let compositor_changed = if last_compositor_poll.elapsed() >= X11_COMPOSITOR_POLL_INTERVAL {
            last_compositor_poll = Instant::now();
            let present = x11_compositor_present(&conn, screen_num);
            let changed = present != compositor_present;
            if changed {
                compositor_present = present;
                // Same reasoning as the RandR repair: what we already painted is
                // still on screen, so blank and let it be repainted before the
                // next root read (and repaint now that the policy changed).
                backdrop.resync(Instant::now());
                tracing::debug!(compositor_present, "X11 overlay compositor state changed");
            }
            changed
        } else {
            false
        };

        let now = Instant::now();
        let elapsed_dt = now.duration_since(last_tick).as_secs_f64();
        last_tick = now;
        let hardware_pointer = conn
            .query_pointer(root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| (f64::from(reply.root_x), f64::from(reply.root_y)));

        // Drain commands and tick.
        let (
            arrived,
            pinned_wid,
            had_msg,
            hover_changed,
            next_frame_tick_needed,
            next_z_order_tick_needed,
            next_idle_wait_interval,
        ) = {
            let mut guard = RENDER.lock().unwrap();
            if let Some(map) = guard.as_mut() {
                if let Some((width, height)) = screen_geometry {
                    update_render_map_geometry(map, width, height);
                }
                let (arrived, had_msg) = process_render_wake(
                    map,
                    first_msg,
                    rx,
                    elapsed_dt,
                    maintenance_timeout,
                    frame_tick_needed,
                );
                let mut hover_changed = false;
                for rs in map.cursors.values_mut() {
                    hover_changed |= rs.core.update_session_badge_hover(hardware_pointer);
                }
                let pinned_wid = map
                    .last_active
                    .as_ref()
                    .and_then(|key| map.cursors.get(key))
                    .and_then(|rs| rs.core.pinned_wid);
                let next_frame_tick_needed = render_map_needs_frame_tick(map);
                let next_z_order_tick_needed = render_map_needs_z_order_tick(map);
                let next_idle_wait_interval = render_map_idle_wait_interval(map);
                (
                    arrived,
                    pinned_wid,
                    had_msg,
                    hover_changed,
                    next_frame_tick_needed,
                    next_z_order_tick_needed,
                    next_idle_wait_interval,
                )
            } else {
                break;
            }
        };

        // Reassert immediately after a command or target change, then at most
        // every 80 ms while any cursor remains visible. Hidden event-service
        // heartbeats neither reassert z-order nor authorize a repaint.
        let pin_changed = pinned_wid != last_pinned;
        last_pinned = pinned_wid;
        if z_order_reassertion_needed(
            had_msg,
            screen_changed,
            pin_changed,
            next_z_order_tick_needed,
            last_ztick.elapsed() >= z_order_interval,
        ) {
            last_ztick = Instant::now();
            // Deliberately does not invalidate the backdrop cache. Restacking
            // *us* does not change what is beneath us, and invalidating here
            // would force a fresh read inside our own silhouette every 80 ms.
            // If a window did draw over us before we raised again, the next
            // root read no longer matches the pixels we uploaded there, so
            // `resolve_backdrop` drops the save-under for those pixels on its
            // own rather than serving a backdrop that is no longer under us.
            z_enforcer.reassert(pinned_wid);
        }

        // Render after a command, during an active animation/fade, and once
        // more as the final active state settles. This leaves the X11 window in
        // its completed/cleared state before the next blocking receive.
        let mut paint_deferred = false;
        if had_msg
            || screen_changed
            || compositor_changed
            || hover_changed
            || frame_tick_needed
            || next_frame_tick_needed
        {
            let tiles = {
                let guard = RENDER.lock().unwrap();
                guard.as_ref().map(render_x11_tiles)
            };

            if let Some(tiles) = tiles {
                match paint_x11_tiles(
                    &conn,
                    &paint_target,
                    &tiles,
                    // A `software_only` window is 24-bit: handing the
                    // compositor branch premultiplied translucent pixels would
                    // display them as opaque dark artifacts, so the downgrade
                    // pins the software-composited path for the session.
                    compositor_present && !software_only,
                    screen_changed,
                    &mut backdrop,
                ) {
                    Ok(outcome) => paint_deferred = outcome == X11PaintOutcome::Deferred,
                    Err(e) => {
                        // A broken X11 connection cannot recover inside this
                        // owner thread. Exit instead of spinning at frame cadence
                        // and repeatedly allocating/rendering work nobody can see.
                        tracing::warn!("X11 overlay paint failed; disabling overlay: {e}");
                        break;
                    }
                }
            }
        }

        // Preserve the original ordering: callers waiting on arrival only
        // resume after the destination frame has reached the X11 window. A
        // deferred paint has not reached it yet, so they wait for the retry.
        pending_arrivals.extend(arrived);
        if !paint_deferred {
            for key in pending_arrivals.drain(..) {
                arrival_fire(&key);
            }
        }

        // A deferred frame is still owed to the window: keep ticking so it is
        // retried instead of leaving a stale cursor until the next command.
        frame_tick_needed = next_frame_tick_needed || paint_deferred;
        maintenance_deadline = next_maintenance_deadline(
            last_tick,
            last_ztick,
            z_order_interval,
            next_z_order_tick_needed,
            next_idle_wait_interval,
            X11_EVENT_POLL_INTERVAL,
        );
        if frame_tick_needed {
            let elapsed = Instant::now().duration_since(last_tick);
            if let Some(remaining) = frame_dur.checked_sub(elapsed) {
                std::thread::sleep(remaining);
            }
        }
    }

    conn.free_gc(gc_id).ok();
    conn.flush().ok();
}

// ── Z-order enforcer (Linux impl of cursor_overlay::ZOrderEnforcer) ──────

/// X11 implementation of [`cursor_overlay::ZOrderEnforcer`].
///
/// Borrows the X11 connection and overlay window id; lives only inside
/// `run_overlay_thread` (the X11 connection is not `'static`). Reasserted after
/// commands, target/display changes, and at most every 80 ms while visible.
#[cfg(target_os = "linux")]
struct X11ZOrderEnforcer<'a, C: x11rb::connection::Connection> {
    conn: &'a C,
    win: u32,
    root: u32,
}

/// Resolve an arbitrary X11 window to its top-level ancestor — the member of
/// its parent chain that is a direct child of `root`.
///
/// `ConfigureWindow` with a `sibling` requires the sibling to share the
/// configured window's parent. The overlay is an override-redirect child of
/// the root, but under a reparenting WM (xfwm4, Mutter, KWin, …) the pinned
/// client window sits inside a WM frame window; restacking against the client
/// XID directly draws BadMatch. The frame — its root-child ancestor — is the
/// window that actually occupies a stacking slot, so it is the correct
/// sibling. Returns `None` for a dead window or a chain that never reaches
/// the root (also proving liveness, replacing a separate attribute probe).
#[cfg(target_os = "linux")]
fn resolve_x11_root_child(
    conn: &impl x11rb::connection::Connection,
    root: u32,
    xid: u32,
) -> Option<u32> {
    use x11rb::protocol::xproto::ConnectionExt as XprotoConnectionExt;
    let mut current = xid;
    // A parent chain deeper than this is not a plausible WM frame tree.
    for _ in 0..32 {
        let tree = conn.query_tree(current).ok()?.reply().ok()?;
        if tree.root != root {
            return None;
        }
        if tree.parent == root || tree.parent == x11rb::NONE {
            return Some(current);
        }
        current = tree.parent;
    }
    None
}

#[cfg(target_os = "linux")]
impl<'a, C: x11rb::connection::Connection> ZOrderEnforcer for X11ZOrderEnforcer<'a, C> {
    fn reassert(&self, target: Option<u64>) {
        use x11rb::protocol::xproto::{
            ConfigureWindowAux, ConnectionExt as XprotoConnectionExt, StackMode,
        };

        // Per the ZOrderEnforcer trait contract, a stale `target` (window
        // gone) should fall back to the `None` behavior — top of the
        // normal stack, no sibling. Using a stale XID as a `sibling` here
        // triggers BadWindow on every tick of the overlay-enforcer loop
        // (~125 Hz), spamming the X server and silently skipping the
        // intended z-reassertion. Resolving the root-child ancestor both
        // probes liveness and yields the only sibling the server will
        // accept under a reparenting WM (see resolve_x11_root_child).
        let target_live =
            target.and_then(|xid| resolve_x11_root_child(self.conn, self.root, xid as u32));
        let aux = if let Some(target_xid) = target_live {
            // Place overlay just above the pinned window's top-level frame.
            ConfigureWindowAux::new()
                .sibling(target_xid)
                .stack_mode(StackMode::ABOVE)
        } else {
            // No pin (or stale target XID) → raise to the top of the
            // normal stack. (X11 has no OS-level "always-on-top" band like
            // Windows / NSStatusWindowLevel, so a plain ABOVE here cannot
            // accidentally float over a focused foreground app the way
            // HWND_TOPMOST would on Windows.)
            ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)
        };
        self.conn.configure_window(self.win, &aux).ok();
        self.conn.flush().ok();
    }
}

#[cfg(target_os = "linux")]
fn find_argb_visual(
    _conn: &impl x11rb::connection::Connection,
    screen: &x11rb::protocol::xproto::Screen,
) -> Option<(u32, u8, u32)> {
    use x11rb::protocol::xproto::VisualClass;
    // Walk all depth entries looking for a 32-bit ARGB visual.
    for depth_entry in &screen.allowed_depths {
        if depth_entry.depth != 32 {
            continue;
        }
        for visual in &depth_entry.visuals {
            if visual.class == VisualClass::TRUE_COLOR {
                return Some((visual.visual_id, 32, screen.default_colormap));
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn x11_compositor_present(conn: &impl x11rb::connection::Connection, screen_num: usize) -> bool {
    use x11rb::protocol::xproto::ConnectionExt as XprotoConnectionExt;

    let selection = format!("_NET_WM_CM_S{screen_num}");
    let Ok(atom_cookie) = conn.intern_atom(false, selection.as_bytes()) else {
        return false;
    };
    let Ok(atom_reply) = atom_cookie.reply() else {
        return false;
    };
    let Ok(owner_cookie) = conn.get_selection_owner(atom_reply.atom) else {
        return false;
    };
    owner_cookie
        .reply()
        .map(|reply| reply.owner != x11rb::NONE)
        .unwrap_or(false)
}

/// Current Linux cursor visuals fit inside a 128×128 logical-pixel tile:
/// bloom/click effects peak below 40 px from the anchor and built-in/custom
/// silhouettes render at 26 px. Keep generous headroom so active work stays
/// independent of the root-window area without clipping antialiasing.
#[cfg(target_os = "linux")]
const X11_CURSOR_TILE_MARGIN: f64 = 64.0;
#[cfg(target_os = "linux")]
const X11_BADGED_CURSOR_HORIZONTAL_MARGIN: f64 =
    cursor_overlay::session_badge_extents().horizontal as f64 + 2.0;

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct X11TileBounds {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
}

#[cfg(target_os = "linux")]
struct X11PaintTile {
    bounds: X11TileBounds,
    pixmap: tiny_skia::Pixmap,
}

/// Loop-constant X11 handles the paint path needs. Bundled so the per-frame
/// paint entry point keeps a reviewable argument list as the compositing
/// inputs grow.
#[cfg(target_os = "linux")]
struct X11PaintTarget {
    win: u32,
    root: u32,
    depth: u8,
    gc_id: u32,
}

/// One rect of screen the overlay painted over: the desktop pixels that were
/// under it, and the pixels we actually put on top of them. Both are row-major
/// BGRX, `width * height * 4` bytes, exactly as XGetImage returns a ZPixmap
/// from the root.
///
/// The pair is what makes readback decidable. A later root read of the same
/// rect is still our own paint when it equals `uploaded`, in which case the
/// true backdrop is `under`; once it differs, the window that owns those pixels
/// has serviced its Expose and the live read is the truth again.
#[cfg(target_os = "linux")]
struct X11SavedBackdrop {
    bounds: X11TileBounds,
    under: Vec<u8>,
    uploaded: Vec<u8>,
}

/// One painted frame: the root-space rects the overlay actually made visible,
/// and the save-unders covering them.
#[cfg(target_os = "linux")]
struct X11BackdropFrame {
    /// When a later frame superseded this one. `None` while the frame is the
    /// one on screen, which can never be read back as backdrop.
    vacated_at: Option<Instant>,
    painted: Vec<x11rb::protocol::xproto::Rectangle>,
    saved: Vec<X11SavedBackdrop>,
}

#[cfg(target_os = "linux")]
impl X11BackdropFrame {
    /// A vacated frame is dropped once its save-unders are old enough that
    /// serving them would be worse than reading whatever is there now. Nothing
    /// about correctness rides on the exact value: while the frame is retained,
    /// `resolve_backdrop` still prefers the live read for every pixel its owner
    /// has repainted.
    fn expired(&self, now: Instant) -> bool {
        match self.vacated_at {
            None => self.painted.is_empty(),
            Some(vacated) => now.duration_since(vacated) > X11_BACKDROP_RETENTION,
        }
    }
}

/// Save-unders for compositor-less software blending. Without a compositing
/// manager the server never blends our alpha, so we blend against the real
/// backdrop ourselves. XGetImage on the root returns our own painted pixels
/// wherever the overlay is currently shaped in, so those pixels are served from
/// here instead and everything else keeps the live read.
#[cfg(target_os = "linux")]
#[derive(Default)]
struct X11BackdropCache {
    /// Newest frame at the front.
    frames: VecDeque<X11BackdropFrame>,
    /// Set while the overlay must stay blank so the windows it uncovered can
    /// service their Expose before the next root read.
    resync_until: Option<Instant>,
    /// Whether the blanking shape for the current resync has been sent.
    resync_blanked: bool,
    slow_reads: u8,
    unbacked_frames: u8,
    disabled: bool,
    /// Set when the startup probe found that a root `GetImage` inside the
    /// overlay's own shaped-in footprint does not return the pixels we
    /// uploaded (xorgxrdp answers black there for a 32-bit ARGB child on a
    /// 24-bit root). On such a server the readback comparison in
    /// `resolve_backdrop` can never confirm our paint survived, so mismatches
    /// inside a save-under serve the saved backdrop instead of adopting the
    /// unreadable live pixels — compositing each frame over those drives a
    /// translucent shadow geometrically to black.
    readback_untrusted: bool,
}

/// After the cache is dropped wholesale — a display change, a compositing
/// manager appearing or leaving — nothing is left to tell our own pixels apart
/// from the desktop, so the overlay blanks itself for this long and lets the
/// windows underneath repaint before it reads the root again. Emptying the
/// bounding shape only queues Expose; the framebuffer still physically holds
/// our last frame until the owning clients answer it.
#[cfg(target_os = "linux")]
const X11_BACKDROP_RESYNC_GRACE: Duration = Duration::from_millis(200);
/// How long a vacated frame's save-under stays available. Long is safe:
/// `resolve_backdrop` only uses it for pixels that still hold our own paint.
#[cfg(target_os = "linux")]
const X11_BACKDROP_RETENTION: Duration = Duration::from_secs(2);
/// Memory ceiling on retained frames, ~2.3 MB per cursor at the largest badged
/// tile. This is what actually bounds the history during continuous motion:
/// 32 frames is roughly half a second at the 60 Hz frame budget.
#[cfg(target_os = "linux")]
const X11_BACKDROP_MAX_FRAMES: usize = 32;
/// A frame's whole root read (one round trip, all tiles) must fit well inside
/// the 16 ms frame budget.
#[cfg(target_os = "linux")]
const X11_BACKDROP_READ_BUDGET: Duration = Duration::from_millis(8);
/// Consecutive over-budget frames that latch software compositing off.
#[cfg(target_os = "linux")]
const X11_BACKDROP_SLOW_READ_LIMIT: u8 = 3;
/// Consecutive frames the overlay may hold still waiting for a usable backdrop
/// before compositing is latched off for the session. Three frames is ~50 ms of
/// frozen cursor, which is cheaper than either painting a tile we cannot
/// account for or losing translucency to a single transient X11 error.
#[cfg(target_os = "linux")]
const X11_BACKDROP_UNBACKED_FRAME_LIMIT: u8 = 3;

/// Software compositing is compositor-less only, and only until a slow server
/// latches it off. Split out so the hard constraint — a compositing manager
/// changes nothing about this path — is locked by a test.
#[cfg(target_os = "linux")]
fn backdrop_compositing_enabled(compositor_present: bool, cache: &X11BackdropCache) -> bool {
    !compositor_present && cache.enabled()
}

/// Intersect a root-space rect with a tile, returning tile-local
/// `(x0, y0, x1, y1)` half-open bounds, or `None` when they do not overlap.
#[cfg(target_os = "linux")]
fn clip_to_tile(
    bounds: X11TileBounds,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Option<(usize, usize, usize, usize)> {
    let bx = i32::from(bounds.x);
    let by = i32::from(bounds.y);
    let x0 = x.max(bx);
    let y0 = y.max(by);
    let x1 = (x + width).min(bx + i32::from(bounds.width));
    let y1 = (y + height).min(by + i32::from(bounds.height));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((
        (x0 - bx) as usize,
        (y0 - by) as usize,
        (x1 - bx) as usize,
        (y1 - by) as usize,
    ))
}

/// Tile-local half-open bounding box of a shape, or `None` when it is empty.
#[cfg(target_os = "linux")]
fn shape_bounding_box(
    shape: &[x11rb::protocol::xproto::Rectangle],
) -> Option<(usize, usize, usize, usize)> {
    let mut bbox: Option<(usize, usize, usize, usize)> = None;
    for rect in shape {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let x0 = rect.x.max(0) as usize;
        let y0 = rect.y.max(0) as usize;
        let x1 = x0 + usize::from(rect.width);
        let y1 = y0 + usize::from(rect.height);
        bbox = Some(match bbox {
            None => (x0, y0, x1, y1),
            Some((bx0, by0, bx1, by1)) => (bx0.min(x0), by0.min(y0), bx1.max(x1), by1.max(y1)),
        });
    }
    bbox
}

/// Copy the `(x0, y0, x1, y1)` sub-rect out of a tile-sized BGRX buffer. Only
/// the painted bounding box is ever consulted later, so save-unders store that
/// instead of the whole tile.
#[cfg(target_os = "linux")]
fn crop_tile_pixels(
    pixels: &[u8],
    tile_width: usize,
    (x0, y0, x1, y1): (usize, usize, usize, usize),
) -> Vec<u8> {
    let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for row in y0..y1 {
        let start = (row * tile_width + x0) * 4;
        out.extend_from_slice(&pixels[start..start + (x1 - x0) * 4]);
    }
    out
}

#[cfg(target_os = "linux")]
impl X11BackdropCache {
    fn enabled(&self) -> bool {
        !self.disabled
    }

    fn disable(&mut self, reason: &'static str) {
        if self.disabled {
            return;
        }
        self.disabled = true;
        self.frames.clear();
        self.resync_until = None;
        tracing::warn!(
            reason,
            "X11 overlay: falling back to the alpha-cutoff cursor shape for this session"
        );
    }

    /// Latch compositing off after repeated over-budget root reads. A loaded VM
    /// must not pay an unbounded XGetImage round trip every frame forever; the
    /// overlay falls back to the alpha-cutoff path for the rest of the session.
    fn note_read_duration(&mut self, elapsed: Duration) {
        if self.disabled {
            return;
        }
        if elapsed <= X11_BACKDROP_READ_BUDGET {
            self.slow_reads = 0;
            return;
        }
        self.slow_reads = self.slow_reads.saturating_add(1);
        if self.slow_reads >= X11_BACKDROP_SLOW_READ_LIMIT {
            self.disable("root backdrop reads exceed the frame budget");
        }
    }

    /// Record a frame the overlay could not composite. The frame is not painted
    /// at all — painting the cutoff tile instead would make pixels visible that
    /// no save-under can explain, and every later frame overlapping them would
    /// fail for the same reason, latching the cutoff look in silently. Returns
    /// `true` once the failures have persisted long enough that compositing is
    /// latched off and the caller should paint the cutoff tile after all.
    fn note_unbacked_frame(&mut self) -> bool {
        if self.disabled {
            // Already latched off — most likely by the slow-read budget during
            // this very frame. Nothing is owed to the cache; paint the cutoff
            // tile now instead of holding a frame back for nothing.
            return true;
        }
        self.unbacked_frames = self.unbacked_frames.saturating_add(1);
        if self.unbacked_frames < X11_BACKDROP_UNBACKED_FRAME_LIMIT {
            return false;
        }
        self.disable("the root backdrop could not be read for several frames");
        true
    }

    fn note_composited_frame(&mut self) {
        self.unbacked_frames = 0;
    }

    /// Drop every save-under without blanking. Only correct where nothing of
    /// ours is on screen to be read back — the compositing-manager path, which
    /// never paints an opaque tile in the first place.
    fn invalidate(&mut self) {
        self.frames.clear();
    }

    /// Drop every save-under and blank the overlay for the resync grace. Used
    /// when the screen the save-unders describe stops being the screen we are
    /// painting on: a RandR geometry change, or a compositing manager arriving
    /// or leaving.
    fn resync(&mut self, now: Instant) {
        self.frames.clear();
        self.resync_until = Some(now + X11_BACKDROP_RESYNC_GRACE);
        self.resync_blanked = false;
    }

    /// Whether painting is still suppressed. Clears the state once the grace
    /// has elapsed so the next frame reads a root nobody has our pixels on.
    fn resyncing(&mut self, now: Instant) -> bool {
        match self.resync_until {
            Some(deadline) if now < deadline => true,
            Some(_) => {
                self.resync_until = None;
                self.resync_blanked = false;
                false
            }
            None => false,
        }
    }

    /// Whether the caller still has to blank the bounding shape for this
    /// resync. Answering `true` once per resync keeps the wait from re-sending
    /// a shape the server already has.
    fn take_resync_blanking(&mut self) -> bool {
        if self.resync_blanked {
            return false;
        }
        self.resync_blanked = true;
        true
    }

    /// Retire frames that are too old to be worth keeping and cap memory. The
    /// on-screen frame is pinned while it has painted rects: those pixels are
    /// what the server is displaying right now.
    fn prune(&mut self, now: Instant) {
        while self.frames.len() > X11_BACKDROP_MAX_FRAMES {
            self.frames.pop_back();
        }
        while let Some(frame) = self.frames.back() {
            if !frame.expired(now) {
                break;
            }
            self.frames.pop_back();
        }
    }

    fn record_frame(
        &mut self,
        at: Instant,
        painted: Vec<x11rb::protocol::xproto::Rectangle>,
        saved: Vec<X11SavedBackdrop>,
    ) {
        // The frame that was on screen is only vacated now, not when it was
        // painted: a cursor that rested for a minute still leaves its pixels
        // behind for the owning client to repaint after it finally moves.
        if let Some(previous) = self.frames.front_mut() {
            previous.vacated_at.get_or_insert(at);
        }
        self.frames.push_front(X11BackdropFrame {
            vacated_at: None,
            painted,
            saved,
        });
        self.prune(at);
    }

    /// Build the true backdrop for `bounds` out of a live root read plus the
    /// save-unders.
    ///
    /// A pixel the overlay painted is only served from a save-under while the
    /// live read still matches the pixels we uploaded there; as soon as the
    /// owning window repaints, the live read wins again. That check is what
    /// keeps a cursor ghost from being blended in and re-saved frame after
    /// frame, and it needs no assumption about how fast a client answers
    /// Expose.
    ///
    /// Returns `None` when a painted pixel has no save-under at all, so callers
    /// fail closed instead of compositing against pixels that may be our own.
    fn resolve_backdrop(
        &self,
        now: Instant,
        bounds: X11TileBounds,
        fresh: &[u8],
    ) -> Option<Vec<u8>> {
        if self.disabled {
            return None;
        }
        let width = usize::from(bounds.width);
        let height = usize::from(bounds.height);
        let pixels = width.checked_mul(height)?;
        if pixels == 0 || fresh.len() != pixels * 4 {
            return None;
        }

        let mut out = fresh.to_vec();
        // `decided` is the running answer for the whole tile; `pending` is the
        // mask of pixels the frame in hand painted and still owes an answer
        // for. Newest frame first, so the most recent paint over a pixel is the
        // one its live value is compared against.
        let mut decided = vec![false; pixels];
        let mut pending = vec![false; pixels];
        for frame in &self.frames {
            if frame.expired(now) {
                continue;
            }
            let mut painted: Option<(usize, usize, usize, usize)> = None;
            for rect in &frame.painted {
                let Some(clip) = clip_to_tile(
                    bounds,
                    i32::from(rect.x),
                    i32::from(rect.y),
                    i32::from(rect.width),
                    i32::from(rect.height),
                ) else {
                    continue;
                };
                let (x0, y0, x1, y1) = clip;
                for row in y0..y1 {
                    pending[row * width + x0..row * width + x1].fill(true);
                }
                painted = Some(match painted {
                    None => clip,
                    Some((px0, py0, px1, py1)) => {
                        (px0.min(x0), py0.min(y0), px1.max(x1), py1.max(y1))
                    }
                });
            }
            let Some((bx0, by0, bx1, by1)) = painted else {
                continue;
            };

            for saved in &frame.saved {
                let Some((x0, y0, x1, y1)) = clip_to_tile(
                    bounds,
                    i32::from(saved.bounds.x),
                    i32::from(saved.bounds.y),
                    i32::from(saved.bounds.width),
                    i32::from(saved.bounds.height),
                ) else {
                    continue;
                };
                let saved_width = usize::from(saved.bounds.width);
                let expected = saved_width * usize::from(saved.bounds.height) * 4;
                if saved.under.len() != expected || saved.uploaded.len() != expected {
                    continue;
                }
                let dx = i32::from(bounds.x) - i32::from(saved.bounds.x);
                let dy = i32::from(bounds.y) - i32::from(saved.bounds.y);
                for row in y0..y1 {
                    let saved_row = (row as i32 + dy) as usize;
                    for col in x0..x1 {
                        let index = row * width + col;
                        if decided[index] || !pending[index] {
                            continue;
                        }
                        let saved_col = (col as i32 + dx) as usize;
                        let src = (saved_row * saved_width + saved_col) * 4;
                        let dst = index * 4;
                        // Compare colour only: the fourth byte is an unused pad
                        // in the root read and a forced 255 in what we upload.
                        // On a server whose root reads cannot see our own
                        // window (`readback_untrusted`) the comparison carries
                        // no information — the live pixel is unreadable, not
                        // repainted — so the save-under is served regardless.
                        // The cost is that a genuine repaint under the cursor
                        // goes unnoticed until the save-under expires.
                        if self.readback_untrusted
                            || fresh[dst..dst + 3] == saved.uploaded[src..src + 3]
                        {
                            out[dst..dst + 4].copy_from_slice(&saved.under[src..src + 4]);
                        }
                        decided[index] = true;
                    }
                }
            }

            // Clear this frame's mask, and fail closed on any pixel it painted
            // that no save-under covers: trusting the live read there would
            // blend our own cursor in as if it were desktop. Every painted rect
            // of a composited frame lies inside one of its save-unders, so this
            // only trips on a frame recorded without one.
            let mut unbacked = false;
            for row in by0..by1 {
                for col in bx0..bx1 {
                    let index = row * width + col;
                    if pending[index] {
                        unbacked |= !decided[index];
                        pending[index] = false;
                    }
                }
            }
            if unbacked {
                return None;
            }
        }
        Some(out)
    }
}

#[cfg(target_os = "linux")]
fn cursor_tile_bounds(
    core: &RenderStateCore,
    screen_width: u32,
    screen_height: u32,
) -> Option<X11TileBounds> {
    if !core.visible || core.pos.0 < -100.0 || core.idle_alpha < 0.004 {
        return None;
    }

    let screen_width = i32::try_from(screen_width).ok()?;
    let screen_height = i32::try_from(screen_height).ok()?;
    let horizontal_margin = if core.session_badge_is_visible() {
        X11_BADGED_CURSOR_HORIZONTAL_MARGIN
    } else {
        X11_CURSOR_TILE_MARGIN
    };
    let left = (core.pos.0 - horizontal_margin).floor() as i32;
    let top = (core.pos.1 - X11_CURSOR_TILE_MARGIN).floor() as i32;
    let right = (core.pos.0 + horizontal_margin).ceil() as i32;
    let bottom = (core.pos.1 + X11_CURSOR_TILE_MARGIN).ceil() as i32;

    let left = left.clamp(0, screen_width);
    let top = top.clamp(0, screen_height);
    let right = right.clamp(0, screen_width);
    let bottom = bottom.clamp(0, screen_height);
    if right <= left || bottom <= top {
        return None;
    }

    Some(X11TileBounds {
        x: i16::try_from(left).ok()?,
        y: i16::try_from(top).ok()?,
        width: u16::try_from(right - left).ok()?,
        height: u16::try_from(bottom - top).ok()?,
    })
}

#[cfg(target_os = "linux")]
fn render_x11_tiles(map: &RenderMap) -> Vec<X11PaintTile> {
    let mut bounds = Vec::new();
    for rs in map.cursors.values() {
        if let Some(tile) = cursor_tile_bounds(&rs.core, map.scr_w, map.scr_h) {
            if !bounds.contains(&tile) {
                bounds.push(tile);
            }
        }
    }

    bounds
        .into_iter()
        .filter_map(|bounds| {
            let mut pixmap = tiny_skia::Pixmap::new(bounds.width.into(), bounds.height.into())?;
            // Paint every cursor into every intersecting tile. tiny-skia clips
            // automatically, so overlapping cursor tiles carry the same
            // composite pixels regardless of upload order — and identical
            // backdrops, because every root read for a frame is issued before
            // any upload.
            for rs in map.cursors.values() {
                cursor_overlay::paint_cursor(
                    &mut pixmap,
                    &rs.core,
                    f64::from(bounds.x),
                    f64::from(bounds.y),
                    None,
                    1.0,
                );
            }
            Some(X11PaintTile { bounds, pixmap })
        })
        .collect()
}

/// Blit cursor-local tiny-skia tiles to the full-root X11 overlay using
/// XPutImage (ZPixmap). The pixmaps are premultiplied RGBA; X11 ARGB is
/// premultiplied BGRA. XShape hides pixels from previous tile positions, so
/// no full-root clear/upload is required when a cursor moves.
#[cfg(target_os = "linux")]
const NONCOMPOSITED_ALPHA_CUTOFF: u8 = 128;

/// Read the true desktop pixels under every tile of one frame.
///
/// All cookies are issued before any reply is collected, so the whole frame
/// costs one round trip regardless of tile count. Callers must invoke this
/// before the first `put_image` of the frame: that ordering is what lets
/// overlapping tiles read the same pre-frame screen instead of each other's
/// freshly uploaded pixels.
///
/// A failed or unexpected read fails the whole frame. Compositing some tiles
/// and not others would upload two different pixel policies into one bounding
/// shape, so the frame is deferred (or, eventually, compositing is latched off)
/// as a unit.
#[cfg(target_os = "linux")]
fn read_root_backdrops(
    conn: &impl x11rb::connection::Connection,
    root: u32,
    tiles: &[X11PaintTile],
    backdrop: &mut X11BackdropCache,
) -> Option<Vec<Vec<u8>>> {
    use x11rb::protocol::xproto::{ConnectionExt as XprotoConnectionExt, ImageFormat};

    // Tiles are pre-clamped to the screen by `cursor_tile_bounds`, so the rect
    // is in bounds by construction; a stale-geometry race after RandR simply
    // errors out into the deferral below.
    let cookies: Vec<_> = tiles
        .iter()
        .map(|tile| {
            let bounds = tile.bounds;
            conn.get_image(
                ImageFormat::Z_PIXMAP,
                root,
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                !0u32,
            )
            .ok()
        })
        .collect();

    let started = Instant::now();
    let reads: Option<Vec<Vec<u8>>> = cookies
        .into_iter()
        .zip(tiles)
        .map(|(cookie, tile)| {
            let reply = cookie?.reply().ok()?;
            let expected = usize::from(tile.bounds.width) * usize::from(tile.bounds.height) * 4;
            // Same BGRX/4-byte assumption the screen capture path decodes with;
            // anything else is a read failure, not a format to guess at. It is
            // also what makes the readback comparison in `resolve_backdrop`
            // meaningful: an 8-bit-per-channel root returns exactly the bytes
            // we uploaded.
            if !matches!(reply.depth, 24 | 32) || reply.data.len() != expected {
                return None;
            }
            Some(reply.data)
        })
        .collect();
    backdrop.note_read_duration(started.elapsed());
    reads
}

/// Whether this server can genuinely display a window of the given visual and
/// fold it into root `GetImage` reads. Both halves of the compositor-less
/// overlay ride on that: PutImage must reach the screen and *stay* there
/// (xorgxrdp repaints a 32-bit ARGB child as solid black on its 24-bit root
/// whenever the region is refreshed), and `resolve_backdrop` decides "still
/// our paint" versus "someone repainted over us" by comparing a later root
/// read against the uploaded bytes — a comparison that misfires on every
/// pixel when the read returns black instead.
///
/// Probed once per candidate visual at startup with a throwaway window that
/// mirrors the real overlay as closely as possible — full-screen,
/// override-redirect, bounding-shaped down to a 4×1 strip at the origin —
/// because the failure is not visible to a simpler probe: xorgxrdp serves a
/// small unshaped ARGB window's pixels back faithfully and only blacks out
/// shaped regions of a large one when its refresh re-renders them. Map,
/// shape, upload the strip, read it back off the root twice (immediately,
/// and again after the server has had time to reprocess the damage),
/// destroy. The round trip through `get_image` orders the reply after the
/// map and upload on this connection, so no separate sync is needed. Errors
/// report as unreliable: a server that cannot service this exchange cannot
/// service the per-frame paint either.
#[cfg(target_os = "linux")]
fn x11_visual_readback_reliable(
    conn: &impl x11rb::connection::Connection,
    root: u32,
    visual_id: u32,
    depth: u8,
    colormap: u32,
    scr_w: u16,
    scr_h: u16,
) -> anyhow::Result<bool> {
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::{
        ClipOrdering, ConnectionExt as XprotoConnectionExt, CreateGCAux, CreateWindowAux,
        EventMask, ImageFormat, Rectangle, WindowClass,
    };

    const PROBE_WIDTH: u16 = 4;
    // Distinctive opaque colours a desktop corner is unlikely to hold.
    let uploaded: Vec<u8> = (0..u32::from(PROBE_WIDTH))
        .flat_map(|i| {
            let i = i as u8;
            [
                0x35_u8.wrapping_add(i.wrapping_mul(7)),
                0x8C_u8.wrapping_sub(i.wrapping_mul(11)),
                0x5A_u8.wrapping_add(i.wrapping_mul(23)),
                0xFF,
            ]
        })
        .collect();

    let win = conn.generate_id()?;
    conn.create_window(
        depth,
        win,
        root,
        0,
        0,
        scr_w.max(PROBE_WIDTH),
        scr_h.max(1),
        0,
        WindowClass::INPUT_OUTPUT,
        visual_id,
        &CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .colormap(colormap)
            .override_redirect(1u32)
            .event_mask(EventMask::NO_EVENT),
    )?;
    let probe = (|| -> anyhow::Result<bool> {
        let gc = conn.generate_id()?;
        conn.create_gc(gc, win, &CreateGCAux::new())?;
        // Shape before mapping so only the probe strip ever shows, exactly as
        // the real overlay exposes only its painted runs.
        conn.shape_rectangles(
            SO::SET,
            SK::BOUNDING,
            ClipOrdering::UNSORTED,
            win,
            0,
            0,
            &[Rectangle {
                x: 0,
                y: 0,
                width: PROBE_WIDTH,
                height: 1,
            }],
        )?;
        conn.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, win, 0, 0, &[])?;
        conn.map_window(win)?;
        conn.put_image(
            ImageFormat::Z_PIXMAP,
            win,
            gc,
            PROBE_WIDTH,
            1,
            0,
            0,
            0,
            depth,
            &uploaded,
        )?;
        // Two reads: the first is ordered right behind the upload on this
        // connection and catches servers that never see the pixels at all;
        // the second runs after a pause long enough for a deferred-refresh
        // server to reprocess the damage. xorgxrdp passes the first read —
        // the bytes are still in its shadow framebuffer — and only blacks
        // the region out when its update timer re-renders it from the window
        // tree, so an immediate-only probe would wrongly bless the visual.
        let matches_upload = |reply: &x11rb::protocol::xproto::GetImageReply| {
            // Colour only: the fourth byte is an unused pad in the root read.
            matches!(reply.depth, 24 | 32)
                && reply.data.len() == uploaded.len()
                && uploaded
                    .chunks_exact(4)
                    .zip(reply.data.chunks_exact(4))
                    .all(|(ours, live)| ours[..3] == live[..3])
        };
        for pause in [None, Some(Duration::from_millis(80))] {
            if let Some(pause) = pause {
                std::thread::sleep(pause);
            }
            let reply = conn
                .get_image(ImageFormat::Z_PIXMAP, root, 0, 0, PROBE_WIDTH, 1, !0u32)?
                .reply()?;
            if !matches_upload(&reply) {
                conn.free_gc(gc)?;
                return Ok(false);
            }
        }
        conn.free_gc(gc)?;
        Ok(true)
    })();
    // Destroy on every exit so a failed probe cannot leave a stray mapped
    // window at the origin.
    conn.destroy_window(win).ok();
    conn.flush().ok();
    probe
}

/// A composited tile: its bounds, the opaque pixels to upload, the tile-local
/// visible shape, and the backdrop they were blended over.
#[cfg(target_os = "linux")]
type X11CompositedTile = (
    X11TileBounds,
    Vec<u8>,
    Vec<x11rb::protocol::xproto::Rectangle>,
    Vec<u8>,
);

/// Compose every tile of a frame, or none of it.
#[cfg(target_os = "linux")]
fn composite_x11_tiles(
    now: Instant,
    tiles: &[X11PaintTile],
    fresh: &[Vec<u8>],
    backdrop: &X11BackdropCache,
) -> Option<Vec<X11CompositedTile>> {
    tiles
        .iter()
        .zip(fresh)
        .map(|(tile, fresh)| {
            let under = backdrop.resolve_backdrop(now, tile.bounds, fresh)?;
            let (bgra, shape) = composited_bgra_and_visible_shape(&tile.pixmap, &under)?;
            Some((tile.bounds, bgra, shape, under))
        })
        .collect()
}

/// Hide every overlay pixel. Used while a resync waits for the windows we
/// uncovered to repaint: the wait is only useful if we are not painting.
#[cfg(target_os = "linux")]
fn blank_x11_overlay_shape(
    conn: &impl x11rb::connection::Connection,
    win: u32,
    checked_requests: bool,
) -> anyhow::Result<()> {
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::ClipOrdering;

    let cookie = conn.shape_rectangles(
        SO::SET,
        SK::BOUNDING,
        ClipOrdering::UNSORTED,
        win,
        0,
        0,
        &[],
    )?;
    if checked_requests {
        cookie.check()?;
    }
    conn.flush()?;
    Ok(())
}

/// Whether the rendered frame reached the X11 window. `Deferred` means the
/// overlay deliberately left the previous frame on screen because it could not
/// account for the pixels this one would cover; the caller must retry on the
/// next frame tick rather than treating the state as settled.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum X11PaintOutcome {
    Painted,
    Deferred,
}

#[cfg(target_os = "linux")]
fn paint_x11_tiles(
    conn: &impl x11rb::connection::Connection,
    target: &X11PaintTarget,
    tiles: &[X11PaintTile],
    compositor_present: bool,
    checked_requests: bool,
    backdrop: &mut X11BackdropCache,
) -> anyhow::Result<X11PaintOutcome> {
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::{ConnectionExt as XprotoConnectionExt, ImageFormat};

    // Phase A — decide, then read every root rect before any pixel is uploaded.
    // Under a compositing manager no root read is issued, no record is written
    // and the cache stays empty: that path is exactly what it was before.
    let now = Instant::now();
    let compositing_available = backdrop_compositing_enabled(compositor_present, backdrop);
    if compositing_available {
        // Retire stale save-unders before they can be consumed, not after.
        backdrop.prune(now);
    } else {
        backdrop.invalidate();
    }

    let mut payloads = Vec::with_capacity(tiles.len());
    let mut visible_shape = Vec::new();
    let mut saved = Vec::new();
    let mut composited = false;

    // Phase B — assemble the true backdrop per tile and blend, or hold the
    // previous frame until we can.
    if compositing_available && !tiles.is_empty() {
        if backdrop.resyncing(now) {
            // Nothing left describes what is under us. Stay invisible until the
            // windows we uncovered have answered their Expose; painting now
            // would recontaminate the region we are waiting on and bake our own
            // cursor into the first save-under.
            if backdrop.take_resync_blanking() {
                blank_x11_overlay_shape(conn, target.win, checked_requests)?;
            }
            return Ok(X11PaintOutcome::Deferred);
        }

        match read_root_backdrops(conn, target.root, tiles, backdrop)
            .and_then(|fresh| composite_x11_tiles(now, tiles, &fresh, backdrop))
        {
            Some(frame) => {
                composited = true;
                backdrop.note_composited_frame();
                for (bounds, bgra, mut tile_shape, under) in frame {
                    // Only the painted bounding box is ever consulted again, so
                    // that is all the frame keeps.
                    if let Some(bbox) = shape_bounding_box(&tile_shape) {
                        let width = usize::from(bounds.width);
                        saved.push(X11SavedBackdrop {
                            bounds: X11TileBounds {
                                x: bounds.x.saturating_add(bbox.0 as i16),
                                y: bounds.y.saturating_add(bbox.1 as i16),
                                width: (bbox.2 - bbox.0) as u16,
                                height: (bbox.3 - bbox.1) as u16,
                            },
                            under: crop_tile_pixels(&under, width, bbox),
                            uploaded: crop_tile_pixels(&bgra, width, bbox),
                        });
                    }
                    for rect in &mut tile_shape {
                        rect.x = rect.x.saturating_add(bounds.x);
                        rect.y = rect.y.saturating_add(bounds.y);
                    }
                    visible_shape.extend(tile_shape);
                    payloads.push((bounds, bgra));
                }
            }
            // Hold the previous frame rather than paint pixels no save-under
            // can explain: doing that once guarantees the next frame fails the
            // same way, which is how a single transient X11 error would
            // otherwise latch the cutoff look in for the session. Once the
            // failures persist, compositing is latched off deliberately and
            // this frame paints the cutoff tile after all.
            None if !backdrop.note_unbacked_frame() => {
                return Ok(X11PaintOutcome::Deferred);
            }
            None => {}
        }
    }

    if !composited {
        for tile in tiles {
            let (bgra, mut tile_shape) = bgra_and_visible_shape(&tile.pixmap, compositor_present);
            for rect in &mut tile_shape {
                rect.x = rect.x.saturating_add(tile.bounds.x);
                rect.y = rect.y.saturating_add(tile.bounds.y);
            }
            visible_shape.extend(tile_shape);
            payloads.push((tile.bounds, bgra));
        }
    }

    // Phase C — expose the new shape, then upload into it. The order matters
    // and is the reverse of what it once was: output to a window is CLIPPED to
    // its current bounding shape, so uploading first silently discards every
    // pixel of the frame's frontier — the runs the new shape adds over the old
    // one — and enlarging the shape afterwards exposes the window's zero
    // backing there instead. On an ARGB window zero is transparent and the
    // loss was invisible, but it left the frontier unreadable to the
    // save-under readback (adopting black as backdrop, which composites the
    // shadow into a hard black ring), and on a 24-bit window zero shows as
    // literal black. Shaping first means the frontier briefly exposes zero
    // backing server-side, but both requests travel in one batch ahead of a
    // single flush, so no frame with the bare frontier is ever presented.
    let shape_cookie = conn.shape_rectangles(
        SO::SET,
        SK::BOUNDING,
        x11rb::protocol::xproto::ClipOrdering::UNSORTED,
        target.win,
        0,
        0,
        &visible_shape,
    )?;
    if checked_requests {
        shape_cookie.check()?;
    }

    // Phase D — upload. A composited buffer covers the whole tile rect; pixels
    // outside the shape are the backdrop copied back and are never displayed.
    for (bounds, bgra) in payloads {
        let cookie = conn.put_image(
            ImageFormat::Z_PIXMAP,
            target.win,
            target.gc_id,
            bounds.width,
            bounds.height,
            bounds.x,
            bounds.y,
            0,
            target.depth,
            &bgra,
        )?;
        if checked_requests {
            cookie.check()?;
        }
    }

    conn.flush()?;

    // Phase E — record what actually landed, after the flush succeeded. The
    // rects are exactly what we SET, so the contamination record is truthful. A
    // clearing paint (no tiles) records an empty frame, which vacates the frame
    // that was on screen without discarding its save-unders: the trail it left
    // is still there until its owners repaint it.
    if compositing_available && backdrop.enabled() && (composited || tiles.is_empty()) {
        backdrop.record_frame(Instant::now(), visible_shape, saved);
    }
    Ok(X11PaintOutcome::Painted)
}

/// Software-composite a premultiplied cursor tile over the real backdrop under
/// it. Without a compositing manager the server never blends our alpha, so we
/// blend here and upload opaque pixels: translucent bloom and antialiased edges
/// survive instead of being quantized to XShape's one bit of visibility. The
/// visible region is still the α≠0 runs, exactly as on the compositing path —
/// at the shape boundary α≈0, so the blended pixel is the backdrop and the
/// 1-bit edge is invisible.
///
/// `backdrop` is a tile-sized row-major BGRX buffer as XGetImage returns it for
/// the root. Returns `None` when it does not match the tile so callers fail
/// closed onto the alpha-cutoff path rather than uploading a guess.
#[cfg(target_os = "linux")]
fn composited_bgra_and_visible_shape(
    pm: &tiny_skia::Pixmap,
    backdrop: &[u8],
) -> Option<(Vec<u8>, Vec<x11rb::protocol::xproto::Rectangle>)> {
    use x11rb::protocol::xproto::Rectangle;

    let width = pm.width() as usize;
    let height = pm.height() as usize;
    let src = pm.data();
    if backdrop.len() != src.len() || backdrop.len() != width * height * 4 {
        return None;
    }

    let mut bgra = Vec::with_capacity(src.len());
    let mut rectangles = Vec::new();

    for y in 0..height {
        let mut run_start = None;
        for x in 0..width {
            let offset = (y * width + x) * 4;
            let pixel = &src[offset..offset + 4];
            let under = &backdrop[offset..offset + 4];
            let alpha = pixel[3];
            let inverse = 255 - u32::from(alpha);
            // Premultiplied source over an opaque backdrop; the result is
            // opaque, so alpha carries no information the server has to blend.
            let over = |source: u8, back: u8| {
                source.saturating_add(((u32::from(back) * inverse + 127) / 255) as u8)
            };
            bgra.extend_from_slice(&[
                over(pixel[2], under[0]),
                over(pixel[1], under[1]),
                over(pixel[0], under[2]),
                255,
            ]);

            if alpha != 0 {
                run_start.get_or_insert(x);
            } else if let Some(start) = run_start.take() {
                rectangles.push(Rectangle {
                    x: start as i16,
                    y: y as i16,
                    width: (x - start) as u16,
                    height: 1,
                });
            }
        }
        if let Some(start) = run_start {
            rectangles.push(Rectangle {
                x: start as i16,
                y: y as i16,
                width: (width - start) as u16,
                height: 1,
            });
        }
    }

    Some((bgra, rectangles))
}

#[cfg(target_os = "linux")]
fn bgra_and_visible_shape(
    pm: &tiny_skia::Pixmap,
    compositor_present: bool,
) -> (Vec<u8>, Vec<x11rb::protocol::xproto::Rectangle>) {
    use x11rb::protocol::xproto::Rectangle;

    let width = pm.width() as usize;
    let height = pm.height() as usize;
    let src = pm.data();
    let mut bgra = Vec::with_capacity(src.len());
    let mut rectangles = Vec::new();

    for y in 0..height {
        let mut run_start = None;
        for x in 0..width {
            let offset = (y * width + x) * 4;
            let pixel = &src[offset..offset + 4];
            let alpha = pixel[3];
            let visible = if compositor_present {
                alpha != 0
            } else {
                // XShape has binary visibility and cannot reproduce the
                // translucent bloom without a compositor. Drop low-alpha
                // bloom pixels, then make retained premultiplied pixels opaque
                // so they do not appear as a dark disk on bare X11.
                alpha >= NONCOMPOSITED_ALPHA_CUTOFF
            };

            if compositor_present || !visible {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], alpha]);
            } else {
                let unpremultiply = |channel: u8| {
                    (((channel as u32 * 255) + (alpha as u32 / 2)) / alpha as u32).min(255) as u8
                };
                bgra.extend_from_slice(&[
                    unpremultiply(pixel[2]),
                    unpremultiply(pixel[1]),
                    unpremultiply(pixel[0]),
                    255,
                ]);
            }

            if visible {
                run_start.get_or_insert(x);
            } else if let Some(start) = run_start.take() {
                rectangles.push(Rectangle {
                    x: start as i16,
                    y: y as i16,
                    width: (x - start) as u16,
                    height: 1,
                });
            }
        }
        if let Some(start) = run_start {
            rectangles.push(Rectangle {
                x: start as i16,
                y: y as i16,
                width: (width - start) as u16,
                height: 1,
            });
        }
    }

    (bgra, rectangles)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn drain_x11_test_events(conn: &impl x11rb::connection::Connection) -> anyhow::Result<()> {
        while conn.poll_for_event()?.is_some() {}
        Ok(())
    }

    fn assert_x11_button_press_target(
        conn: &impl x11rb::connection::Connection,
        root: u32,
        expected_window: u32,
    ) -> anyhow::Result<()> {
        use x11rb::protocol::xproto::{
            BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, MOTION_NOTIFY_EVENT,
        };
        use x11rb::protocol::xtest::ConnectionExt as XtestConnectionExt;

        drain_x11_test_events(conn)?;
        conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 0, x11rb::CURRENT_TIME, root, 50, 50, 0)?
            .check()?;
        conn.xtest_fake_input(BUTTON_PRESS_EVENT, 1, x11rb::CURRENT_TIME, root, 0, 0, 0)?
            .check()?;
        conn.xtest_fake_input(BUTTON_RELEASE_EVENT, 1, x11rb::CURRENT_TIME, root, 0, 0, 0)?
            .check()?;
        conn.flush()?;

        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if let Some(event) = conn.poll_for_event()? {
                if let x11rb::protocol::Event::ButtonPress(button) = event {
                    anyhow::ensure!(
                        button.event == expected_window,
                        "button press reached window {} instead of expected window {}",
                        button.event,
                        expected_window
                    );
                    return Ok(());
                }
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        anyhow::bail!("timed out waiting for an X11 button press")
    }

    #[test]
    #[ignore = "requires a live X11 server with XTEST, Shape, and RandR (run under xvfb-run or XWayland)"]
    fn x11_overlay_owner_stays_click_through_across_cursor_lifecycle_and_randr(
    ) -> anyhow::Result<()> {
        use x11rb::connection::Connection;
        use x11rb::protocol::randr::ConnectionExt as RandrConnectionExt;
        use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK};
        use x11rb::protocol::xproto::{
            AtomEnum, ConnectionExt as XprotoConnectionExt, CreateWindowAux, EventMask, MapState,
            Window, WindowClass,
        };

        fn find_named_window(
            conn: &impl Connection,
            root: Window,
            expected_name: &[u8],
        ) -> anyhow::Result<Window> {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                for window in conn.query_tree(root)?.reply()?.children {
                    let name = conn
                        .get_property(
                            false,
                            window,
                            AtomEnum::WM_NAME,
                            AtomEnum::STRING,
                            0,
                            u32::MAX,
                        )?
                        .reply()?;
                    if name.value == expected_name {
                        return Ok(window);
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            anyhow::bail!(
                "timed out waiting for X11 window {}",
                String::from_utf8_lossy(expected_name)
            )
        }

        fn wait_for_bounding_shape(
            conn: &impl Connection,
            overlay: Window,
            should_be_empty: bool,
            phase: &str,
        ) -> anyhow::Result<()> {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                let shape = conn.shape_get_rectangles(overlay, SK::BOUNDING)?.reply()?;
                if shape.rectangles.is_empty() == should_be_empty {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            anyhow::bail!("overlay bounding shape did not settle during {phase}")
        }

        fn assert_click_through(
            conn: &impl Connection,
            root: Window,
            overlay: Window,
            target: Window,
            phase: &str,
        ) -> anyhow::Result<()> {
            let attributes = conn.get_window_attributes(overlay)?.reply()?;
            anyhow::ensure!(
                attributes.map_state == MapState::VIEWABLE,
                "overlay was not mapped during {phase}"
            );
            let input = conn.shape_get_rectangles(overlay, SK::INPUT)?.reply()?;
            anyhow::ensure!(
                input.rectangles.is_empty(),
                "overlay exposed {} ShapeInput rectangles during {phase}",
                input.rectangles.len()
            );
            assert_x11_button_press_target(conn, root, target).map_err(|error| {
                anyhow::anyhow!("click delivery failed during {phase}: {error}")
            })?;
            eprintln!(
                "{phase}: ShapeInput rectangles={}, click_target={target}",
                input.rectangles.len()
            );
            Ok(())
        }

        fn wait_for_cursor_move_from(x: f64, y: f64, phase: &str) -> anyhow::Result<()> {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                let position = current_position();
                if (position.0 - x).hypot(position.1 - y) > 32.0 {
                    eprintln!(
                        "{phase}: cursor moved from ({x}, {y}) to ({}, {})",
                        position.0, position.1
                    );
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let position = current_position();
            anyhow::bail!(
                "cursor position remained ({}, {}) near ({x}, {y}) during {phase}",
                position.0,
                position.1
            )
        }

        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let root_depth = screen.root_depth;
        let root_visual = screen.root_visual;
        let initial_width = screen.width_in_pixels;
        let initial_height = screen.height_in_pixels;
        let initial_mm_width = screen.width_in_millimeters;
        let initial_mm_height = screen.height_in_millimeters;

        let target = conn.generate_id()?;
        conn.create_window(
            root_depth,
            target,
            root,
            0,
            0,
            400,
            400,
            0,
            WindowClass::INPUT_OUTPUT,
            root_visual,
            &CreateWindowAux::new().event_mask(EventMask::BUTTON_PRESS),
        )?
        .check()?;
        conn.map_window(target)?.check()?;
        conn.flush()?;

        let cursor_id = "issue-1819-live-shape-probe";
        let cfg = CursorConfig {
            cursor_id: cursor_id.to_owned(),
            ..CursorConfig::default()
        };
        init(cfg);
        run_on_thread();

        let title = format!("Cua.AgentCursorOverlay.{cursor_id}");
        let overlay = find_named_window(&conn, root, title.as_bytes())?;
        wait_for_bounding_shape(&conn, overlay, true, "daemon startup")?;
        assert_click_through(&conn, root, overlay, target, "daemon startup")?;

        send_command(OverlayCommand::SetEnabled(true));
        send_command(OverlayCommand::SnapTo {
            x: 160.0,
            y: 160.0,
            heading_radians: None,
        });
        wait_for_bounding_shape(&conn, overlay, false, "cursor show")?;
        assert_click_through(&conn, root, overlay, target, "cursor show")?;

        send_command(OverlayCommand::MoveTo {
            x: 240.0,
            y: 240.0,
            end_heading_radians: std::f64::consts::FRAC_PI_4,
        });
        wait_for_cursor_move_from(160.0, 160.0, "cursor move")?;
        wait_for_bounding_shape(&conn, overlay, false, "cursor move")?;
        assert_click_through(&conn, root, overlay, target, "cursor move")?;

        send_command(OverlayCommand::SetEnabled(false));
        wait_for_bounding_shape(&conn, overlay, true, "cursor hide")?;
        assert_click_through(&conn, root, overlay, target, "cursor hide")?;

        send_command(OverlayCommand::SetEnabled(true));
        send_command(OverlayCommand::SnapTo {
            x: 160.0,
            y: 160.0,
            heading_radians: None,
        });
        wait_for_bounding_shape(&conn, overlay, false, "cursor reshow")?;

        let repairs_before = X11_RANDR_REPAIR_COUNT.load(std::sync::atomic::Ordering::SeqCst);
        conn.randr_set_screen_size(
            root,
            initial_width,
            initial_height,
            u32::from(initial_mm_width).saturating_add(1),
            u32::from(initial_mm_height).saturating_add(1),
        )?
        .check()?;
        conn.flush()?;

        let deadline = Instant::now() + Duration::from_secs(3);
        let repairs_after = loop {
            let repairs = X11_RANDR_REPAIR_COUNT.load(std::sync::atomic::Ordering::SeqCst);
            if repairs > repairs_before {
                break repairs;
            }
            if Instant::now() >= deadline {
                anyhow::bail!("overlay owner did not repair after the RandR configuration event");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let geometry = conn.get_geometry(overlay)?.reply()?;
        anyhow::ensure!(
            geometry.width == initial_width && geometry.height == initial_height,
            "overlay geometry became {}x{} after same-size RandR repair",
            geometry.width,
            geometry.height
        );
        eprintln!(
            "RandR repair: count={repairs_after}, overlay_geometry={}x{}",
            geometry.width, geometry.height
        );
        wait_for_bounding_shape(&conn, overlay, false, "RandR repair")?;
        assert_click_through(&conn, root, overlay, target, "RandR repair")?;
        Ok(())
    }

    #[test]
    #[ignore = "requires a live X11 server (run under xvfb-run)"]
    fn x11_overlay_input_shape_stays_empty_and_clicks_pass_through_after_resize(
    ) -> anyhow::Result<()> {
        use x11rb::connection::Connection;
        use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
        use x11rb::protocol::xproto::{
            ClipOrdering, ConnectionExt as XprotoConnectionExt, CreateWindowAux, EventMask,
            Rectangle, WindowClass,
        };

        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let target = conn.generate_id()?;
        let overlay = conn.generate_id()?;

        conn.create_window(
            screen.root_depth,
            target,
            root,
            0,
            0,
            200,
            200,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().event_mask(EventMask::BUTTON_PRESS),
        )?
        .check()?;
        conn.create_window(
            screen.root_depth,
            overlay,
            root,
            0,
            0,
            200,
            200,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .event_mask(EventMask::BUTTON_PRESS),
        )?
        .check()?;
        conn.map_window(target)?.check()?;
        conn.map_window(overlay)?.check()?;
        conn.flush()?;

        let initial_input = conn.shape_get_rectangles(overlay, SK::INPUT)?.reply()?;
        assert!(
            !initial_input.rectangles.is_empty(),
            "fixture must begin with a full input region"
        );
        assert_x11_button_press_target(&conn, root, overlay)?;

        conn.unmap_window(overlay)?.check()?;
        map_x11_overlay_with_empty_input(&conn, overlay)?;
        let click_through_input = conn.shape_get_rectangles(overlay, SK::INPUT)?.reply()?;
        assert!(
            click_through_input.rectangles.is_empty(),
            "click-through overlay must expose an empty X11 input shape"
        );
        let initial_visible_bounds = [Rectangle {
            x: 0,
            y: 0,
            width: 200,
            height: 200,
        }];
        conn.shape_rectangles(
            SO::SET,
            SK::BOUNDING,
            ClipOrdering::UNSORTED,
            overlay,
            0,
            0,
            &initial_visible_bounds,
        )?
        .check()?;
        conn.flush()?;
        assert_x11_button_press_target(&conn, root, target)?;

        prepare_x11_overlay_geometry(
            &conn,
            overlay,
            screen.width_in_pixels,
            screen.height_in_pixels,
        )?;
        let full_root = [Rectangle {
            x: 0,
            y: 0,
            width: screen.width_in_pixels,
            height: screen.height_in_pixels,
        }];
        conn.shape_rectangles(
            SO::SET,
            SK::BOUNDING,
            ClipOrdering::UNSORTED,
            overlay,
            0,
            0,
            &full_root,
        )?
        .check()?;
        conn.flush()?;

        let repaired_input = conn.shape_get_rectangles(overlay, SK::INPUT)?.reply()?;
        assert!(
            repaired_input.rectangles.is_empty(),
            "resize repair must not restore a stale full-root input region"
        );
        assert_x11_button_press_target(&conn, root, target)?;

        let missing_window = conn.generate_id()?;
        assert!(
            map_x11_overlay_with_empty_input(&conn, missing_window).is_err(),
            "a rejected input-shape request must abort before mapping"
        );
        assert!(
            conn.poll_for_event()?.is_none(),
            "mapping must not be attempted after the checked input-shape request fails"
        );
        Ok(())
    }

    #[test]
    fn keyed_render_state_carries_the_session_color_identity() {
        let state = render_state_for_key(&CursorConfig::default(), "session-blueprint");
        assert_eq!(state.core.cfg.cursor_id, "session-blueprint");
    }

    fn default_render_map() -> RenderMap {
        let cfg = CursorConfig::default();
        let mut cursors = HashMap::new();
        cursors.insert("default".to_owned(), RenderState::new(cfg.clone()));
        RenderMap {
            cursors,
            scr_w: 100,
            scr_h: 100,
            template: cfg,
            ended: HashSet::new(),
            last_active: None,
        }
    }

    #[test]
    fn explicit_revival_clears_tombstone_and_recreates_lazily() {
        let move_msg = |x, y| {
            OverlayMsg::Cmd(KeyedOverlayCommand {
                key: "sessA".to_owned(),
                cmd: OverlayCommand::MoveTo {
                    x,
                    y,
                    end_heading_radians: 0.0,
                },
            })
        };
        let mut map = default_render_map();
        apply_msg(&mut map, move_msg(10.0, 10.0));
        apply_msg(&mut map, OverlayMsg::Remove("sessA".to_owned()));
        assert!(apply_msg(&mut map, move_msg(20.0, 20.0)).is_none());

        apply_msg(&mut map, OverlayMsg::Revive("sessA".to_owned()));
        assert!(!map.cursors.contains_key("sessA"));
        assert!(!map.ended.contains("sessA"));

        let resolved = apply_msg(&mut map, move_msg(30.0, 30.0));
        assert_eq!(resolved.as_deref(), Some("sessA"));
        assert!(map.cursors.contains_key("sessA"));
    }

    fn test_message() -> OverlayMsg {
        OverlayMsg::Cmd(KeyedOverlayCommand {
            key: "default".to_owned(),
            cmd: OverlayCommand::SetEnabled(true),
        })
    }

    #[test]
    fn send_command_for_keeps_unit_returning_api() {
        let _: fn(CursorKey, OverlayCommand) = send_command_for;
    }

    #[test]
    fn failed_x11_enqueue_is_reported_without_waiting() {
        assert!(!try_send_x11_message(None, test_message()));

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        drop(rx);
        assert!(!try_send_x11_message(Some(&tx), test_message()));

        let (tx, _rx) = std::sync::mpsc::sync_channel(0);
        assert!(!try_send_x11_message(Some(&tx), test_message()));
    }

    #[test]
    fn successful_x11_enqueue_is_reported() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        assert!(try_send_x11_message(Some(&tx), test_message()));
        assert!(matches!(rx.try_recv(), Ok(OverlayMsg::Cmd(_))));
    }

    #[test]
    fn teardown_disconnects_before_releasing_registration() {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::sync_channel(1);
        let mut cleanup = X11OverlayThreadCleanup {
            receiver: Some(cmd_rx),
            disable_render_state: false,
        };

        let key = "teardown-race".to_owned();
        let (arrival_tx, mut arrival_rx) = tokio::sync::oneshot::channel();
        cleanup.teardown_with_after_disconnect(|| {
            assert!(matches!(
                cmd_tx.try_send(test_message()),
                Err(std::sync::mpsc::TrySendError::Disconnected(_))
            ));
            arrival_register(key, arrival_tx);
        });
        assert!(matches!(
            arrival_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        ));
    }

    #[test]
    fn clearing_arrivals_releases_waiters() {
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        let mut arrivals = Some(HashMap::from([("default".to_owned(), tx)]));

        clear_arrivals(&mut arrivals);

        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        ));
    }

    #[test]
    fn disabling_render_map_marks_overlay_unavailable() {
        let mut render = Some(default_render_map());
        disable_render_map(&mut render);
        assert!(render.is_none());
    }

    #[test]
    fn active_frame_wake_does_not_consume_queued_command() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(test_message()).unwrap();

        assert!(matches!(
            wait_for_overlay_work(&rx, true, Instant::now()),
            OverlayWake::Frame
        ));
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn idle_wait_wakes_for_command() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(test_message()).unwrap();

        match wait_for_overlay_work(&rx, false, Instant::now()) {
            OverlayWake::Message(OverlayMsg::Cmd(keyed)) => assert_eq!(keyed.key, "default"),
            _ => panic!("idle wait did not return the queued command"),
        }
    }

    #[test]
    fn resting_cursor_wait_times_out_for_maintenance() {
        let (_tx, rx) = std::sync::mpsc::channel();
        assert!(matches!(
            wait_for_overlay_work(&rx, false, Instant::now() + Duration::from_millis(1)),
            OverlayWake::MaintenanceTimeout
        ));
    }

    #[test]
    fn expired_maintenance_deadline_times_out_immediately() {
        let (_tx, rx) = std::sync::mpsc::channel();
        let deadline = Instant::now() - Duration::from_millis(1);
        assert!(matches!(
            wait_for_overlay_work(&rx, false, deadline),
            OverlayWake::MaintenanceTimeout
        ));
    }

    #[test]
    fn idle_deadline_is_anchored_to_the_state_tick() {
        let state_tick = Instant::now();
        let z_order_tick = state_tick + Duration::from_millis(5);
        let deadline = next_maintenance_deadline(
            state_tick,
            z_order_tick,
            Duration::from_millis(80),
            true,
            Some(Duration::from_millis(20)),
            X11_EVENT_POLL_INTERVAL,
        );

        assert_eq!(deadline, state_tick + Duration::from_millis(20));
    }

    #[test]
    fn maintenance_tick_advances_the_full_elapsed_interval() {
        let mut map = default_render_map();
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.pos = (10.0, 10.0);
        cursor.core.motion.idle_hide_ms = 500.0;
        let (_tx, rx) = std::sync::mpsc::channel();

        let (arrived, had_msg) = process_render_wake(&mut map, None, &rx, 0.08, true, false);

        assert!(arrived.is_empty());
        assert!(!had_msg);
        assert_eq!(map.cursors["default"].core.idle_secs, 0.08);
    }

    #[test]
    fn idle_wait_reports_disconnected_sender() {
        let (tx, rx) = std::sync::mpsc::channel();
        drop(tx);
        assert!(matches!(
            wait_for_overlay_work(&rx, false, Instant::now()),
            OverlayWake::Disconnected
        ));
    }

    #[test]
    fn hidden_cursor_deadline_wakes_to_service_x11_events() {
        let state_tick = Instant::now();
        let deadline = next_maintenance_deadline(
            state_tick,
            state_tick,
            Duration::from_millis(80),
            false,
            None,
            X11_EVENT_POLL_INTERVAL,
        );

        assert_eq!(deadline, state_tick + X11_EVENT_POLL_INTERVAL);
    }

    #[test]
    fn hidden_heartbeat_does_not_request_z_order_reassertion() {
        assert!(!z_order_reassertion_needed(
            false, false, false, false, true,
        ));
        assert!(z_order_reassertion_needed(false, false, false, true, true,));
        assert!(!z_order_reassertion_needed(
            false, false, false, true, false,
        ));
        assert!(z_order_reassertion_needed(true, false, false, false, false,));
        assert!(z_order_reassertion_needed(false, true, false, false, false,));
        assert!(z_order_reassertion_needed(false, false, true, false, false,));
    }

    #[test]
    fn x11_event_poll_precedes_slower_visible_cursor_z_order_tick() {
        let state_tick = Instant::now();
        let deadline = next_maintenance_deadline(
            state_tick,
            state_tick,
            Duration::from_millis(80),
            true,
            None,
            X11_EVENT_POLL_INTERVAL,
        );

        assert_eq!(deadline, state_tick + X11_EVENT_POLL_INTERVAL);
    }

    #[test]
    fn randr_display_changes_request_geometry_refresh() {
        let screen_change = x11rb::protocol::Event::RandrScreenChangeNotify(Default::default());
        let crtc_change =
            x11rb::protocol::Event::RandrNotify(x11rb::protocol::randr::NotifyEvent {
                response_type: 0,
                sub_code: x11rb::protocol::randr::Notify::CRTC_CHANGE,
                sequence: 0,
                u: x11rb::protocol::randr::CrtcChange::default().into(),
            });
        assert!(classify_x11_overlay_event(&screen_change, 7).unwrap());
        assert!(classify_x11_overlay_event(&crtc_change, 7).unwrap());
        assert!(!classify_x11_overlay_event(&x11rb::protocol::Event::Unknown(vec![]), 7).unwrap());
    }

    #[test]
    fn x11_server_error_is_fatal() {
        let error = x11rb::protocol::Event::Error(x11rb::x11_utils::X11Error {
            error_kind: x11rb::protocol::ErrorKind::Drawable,
            error_code: 9,
            sequence: 1,
            bad_value: 42,
            minor_opcode: 0,
            major_opcode: 72,
            extension_name: None,
            request_name: Some("PutImage"),
        });

        assert!(classify_x11_overlay_event(&error, 7).is_err());
    }

    #[test]
    fn stale_z_order_sibling_badwindow_is_recoverable() {
        let error = x11rb::protocol::Event::Error(x11rb::x11_utils::X11Error {
            error_kind: x11rb::protocol::ErrorKind::Window,
            error_code: 3,
            sequence: 1,
            bad_value: 42,
            minor_opcode: 0,
            major_opcode: x11rb::protocol::xproto::CONFIGURE_WINDOW_REQUEST,
            extension_name: None,
            request_name: Some("ConfigureWindow"),
        });

        assert!(!classify_x11_overlay_event(&error, 7).unwrap());
    }

    #[test]
    fn overlay_badwindow_remains_fatal() {
        let error = x11rb::protocol::Event::Error(x11rb::x11_utils::X11Error {
            error_kind: x11rb::protocol::ErrorKind::Window,
            error_code: 3,
            sequence: 1,
            bad_value: 7,
            minor_opcode: 0,
            major_opcode: x11rb::protocol::xproto::CONFIGURE_WINDOW_REQUEST,
            extension_name: None,
            request_name: Some("ConfigureWindow"),
        });

        assert!(classify_x11_overlay_event(&error, 7).is_err());
    }

    #[test]
    fn z_order_sibling_badmatch_is_recoverable() {
        // A reparenting WM can move the sibling under a frame window between
        // the ancestor resolution and the restack; the server answers with
        // BadMatch. That only means the z-order nudge missed — it must not
        // kill the overlay (it used to: the first PinAbove under xfwm4
        // silently disabled the cursor for the daemon's lifetime).
        let error = x11rb::protocol::Event::Error(x11rb::x11_utils::X11Error {
            error_kind: x11rb::protocol::ErrorKind::Match,
            error_code: 8,
            sequence: 1,
            bad_value: 42,
            minor_opcode: 0,
            major_opcode: x11rb::protocol::xproto::CONFIGURE_WINDOW_REQUEST,
            extension_name: None,
            request_name: Some("ConfigureWindow"),
        });

        assert!(!classify_x11_overlay_event(&error, 7).unwrap());
    }

    #[test]
    fn overlay_badmatch_remains_fatal() {
        // A Match error naming the overlay window itself is not a stale
        // sibling — something is structurally wrong with our own window.
        let error = x11rb::protocol::Event::Error(x11rb::x11_utils::X11Error {
            error_kind: x11rb::protocol::ErrorKind::Match,
            error_code: 8,
            sequence: 1,
            bad_value: 7,
            minor_opcode: 0,
            major_opcode: x11rb::protocol::xproto::CONFIGURE_WINDOW_REQUEST,
            extension_name: None,
            request_name: Some("ConfigureWindow"),
        });

        assert!(classify_x11_overlay_event(&error, 7).is_err());
    }

    #[test]
    fn geometry_update_sets_render_bounds() {
        let mut map = default_render_map();
        update_render_map_geometry(&mut map, 1920, 2160);
        assert_eq!((map.scr_w, map.scr_h), (1920, 2160));
    }

    #[test]
    fn geometry_shrink_reclips_cursor_tiles() {
        let mut map = default_render_map();
        map.scr_w = 1920;
        map.scr_h = 2160;
        map.cursors.get_mut("default").unwrap().core.pos = (100.0, 2000.0);
        assert_eq!(render_x11_tiles(&map).len(), 1);

        update_render_map_geometry(&mut map, 1920, 1080);
        assert!(render_x11_tiles(&map).is_empty());
    }

    #[test]
    fn sentinel_default_cursor_does_not_require_frame_ticks() {
        let map = default_render_map();
        assert!(!render_map_needs_frame_tick(&map));
        assert!(!render_map_needs_z_order_tick(&map));
    }

    // The idle-park contract now applies to reduced-motion sessions: with the
    // float bob active (the default), a visible cursor keeps ticking so it
    // levitates at rest, and only a hidden or reduced-motion cursor parks.
    #[test]
    fn resting_visible_cursor_only_requires_cheap_z_order_ticks() {
        let mut map = default_render_map();
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.pos = (100.0, 100.0);
        cursor.core.motion.idle_hide_ms = 0.0;
        cursor.core.visual.reduced_motion = cursor_overlay::ReducedMotion::On;

        assert!(!render_map_needs_frame_tick(&map));
        assert!(render_map_needs_z_order_tick(&map));
    }

    #[test]
    fn resting_visible_cursor_keeps_ticking_for_the_float_bob() {
        let mut map = default_render_map();
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.pos = (100.0, 100.0);
        cursor.core.motion.idle_hide_ms = 0.0;

        // Default reduced_motion (auto) floats, so frames keep flowing while
        // the cursor is visible…
        assert!(render_map_needs_frame_tick(&map));

        // …and stop once the idle fade has fully hidden it.
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.idle_alpha = 0.0;
        assert!(!render_map_needs_frame_tick(&map));
    }

    #[test]
    fn disabling_settled_cursor_clears_once_then_parks() {
        let mut map = default_render_map();
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.pos = (100.0, 100.0);
        cursor.core.motion.idle_hide_ms = 0.0;
        cursor.core.visual.reduced_motion = cursor_overlay::ReducedMotion::On;
        let (_tx, rx) = std::sync::mpsc::channel();

        assert!(!render_map_needs_frame_tick(&map));
        assert!(render_map_needs_z_order_tick(&map));

        let (arrived, had_msg) = process_render_wake(
            &mut map,
            Some(OverlayMsg::Cmd(KeyedOverlayCommand {
                key: "default".to_owned(),
                cmd: OverlayCommand::SetEnabled(false),
            })),
            &rx,
            0.08,
            false,
            false,
        );

        assert!(arrived.is_empty());
        // The production render gate includes `had_msg`, so disabling paints
        // one final transparent frame before both scheduler paths park.
        assert!(had_msg);
        assert!(!map.cursors["default"].core.visible);
        assert!(!render_map_needs_frame_tick(&map));
        assert!(!render_map_needs_z_order_tick(&map));
    }

    #[test]
    fn active_cursor_requires_frame_ticks() {
        let mut map = default_render_map();
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.apply_command(OverlayCommand::MoveTo {
            x: 250.0,
            y: 150.0,
            end_heading_radians: 0.0,
        });
        assert!(render_map_needs_frame_tick(&map));
    }

    #[test]
    fn completed_move_parks_during_opaque_idle_delay() {
        let mut map = default_render_map();
        let cursor = map.cursors.get_mut("default").unwrap();
        // The public animate path seeds a newly created cursor near its target
        // before sending MoveTo; mirror that valid on-screen starting state.
        cursor.core.pos = (100.0, 100.0);
        cursor.core.motion.idle_hide_ms = 500.0;
        cursor.core.visual.reduced_motion = cursor_overlay::ReducedMotion::On;
        cursor.apply_command(OverlayCommand::MoveTo {
            x: 250.0,
            y: 150.0,
            end_heading_radians: 0.0,
        });

        for _ in 0..1200 {
            cursor.tick(1.0 / 60.0);
            if !cursor.needs_frame_tick() {
                break;
            }
        }

        assert!(
            !cursor.needs_frame_tick(),
            "cursor did not quiesce: path={}, spring={}, click={}, idle_secs={:.3}, idle_alpha={:.3}, pos={:?}",
            cursor.core.path.is_some(),
            cursor.core.spring.is_some(),
            cursor.core.click_t.is_some(),
            cursor.core.idle_secs,
            cursor.core.idle_alpha,
            cursor.core.pos,
        );
        assert_eq!(cursor.core.idle_alpha, 1.0);
        assert!(!render_map_needs_frame_tick(&map));
        assert!(render_map_needs_z_order_tick(&map));
    }

    #[test]
    fn commands_for_one_cursor_do_not_starve_another_cursors_idle_deadline() {
        let mut map = default_render_map();
        {
            let cursor = map.cursors.get_mut("default").unwrap();
            cursor.core.pos = (10.0, 10.0);
            cursor.core.motion.idle_hide_ms = 500.0;
        }
        let other = render_state_for_key(&map.template, "other");
        map.cursors.insert("other".to_owned(), other);
        let (_tx, rx) = std::sync::mpsc::channel();

        // Model five command-channel wakeups at 100 ms intervals. The parked
        // elapsed time must advance all existing cursors before each unrelated
        // command is applied.
        for _ in 0..5 {
            let (arrived, had_msg) = process_render_wake(
                &mut map,
                Some(OverlayMsg::Cmd(KeyedOverlayCommand {
                    key: "other".to_owned(),
                    cmd: OverlayCommand::SetTheme {
                        theme_id: cursor_overlay::DEFAULT_THEME_ID.to_owned(),
                        reduced_motion: cursor_overlay::ReducedMotion::Auto,
                    },
                })),
                &rx,
                0.1,
                false,
                false,
            );
            assert!(arrived.is_empty());
            assert!(had_msg);
        }

        let cursor = map.cursors.get("default").unwrap();
        assert!(cursor.core.idle_secs >= 0.5);
        assert!(cursor.needs_frame_tick());
        assert_eq!(cursor.core.idle_alpha, 1.0);
    }

    #[test]
    fn command_drained_after_maintenance_timeout_starts_at_zero_dt() {
        let mut map = default_render_map();
        {
            let cursor = map.cursors.get_mut("default").unwrap();
            cursor.core.pos = (10.0, 10.0);
            cursor.core.motion.idle_hide_ms = 500.0;
        }
        let mut other = render_state_for_key(&map.template, "other");
        other.core.pos = (20.0, 20.0);
        map.cursors.insert("other".to_owned(), other);

        // Model a command arriving after recv_timeout returned Timeout but
        // before the render loop's try_recv drain.
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(OverlayMsg::Cmd(KeyedOverlayCommand {
            key: "other".to_owned(),
            cmd: OverlayCommand::MoveTo {
                x: 250.0,
                y: 150.0,
                end_heading_radians: 0.0,
            },
        }))
        .unwrap();

        let (arrived, had_msg) = process_render_wake(&mut map, None, &rx, 0.08, true, false);

        assert!(arrived.is_empty());
        assert!(had_msg);
        assert_eq!(map.cursors["default"].core.idle_secs, 0.08);
        let other = &map.cursors["other"].core;
        assert!(other.path.is_some());
        assert_eq!(other.pos, (20.0, 20.0));
        assert_eq!(other.dist, 0.0);
    }

    #[test]
    fn click_pulse_drained_after_maintenance_timeout_starts_at_zero_dt() {
        let mut map = default_render_map();
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.pos = (20.0, 20.0);

        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(OverlayMsg::Cmd(KeyedOverlayCommand {
            key: "default".to_owned(),
            cmd: OverlayCommand::ClickPulse { x: 40.0, y: 50.0 },
        }))
        .unwrap();

        let (arrived, had_msg) = process_render_wake(&mut map, None, &rx, 0.08, true, false);

        assert!(arrived.is_empty());
        assert!(had_msg);
        let cursor = &map.cursors["default"].core;
        assert_eq!(cursor.pos, (40.0, 50.0));
        assert_eq!(cursor.click_t, Some(0.0));
    }

    #[test]
    fn active_frame_replacement_does_not_return_stale_arrival() {
        let mut map = default_render_map();
        {
            let cursor = map.cursors.get_mut("default").unwrap();
            cursor.core.pos = (20.0, 20.0);
            cursor.apply_command(OverlayCommand::MoveTo {
                x: 80.0,
                y: 80.0,
                end_heading_radians: 0.0,
            });
            let old_path_len = cursor.core.path.as_ref().unwrap().length.max(1.0);
            // The old path would finish on the next 16 ms tick if active-frame
            // wakes were globally changed to tick before applying commands.
            cursor.core.dist = old_path_len - 0.001;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(OverlayMsg::Cmd(KeyedOverlayCommand {
            key: "default".to_owned(),
            cmd: OverlayCommand::MoveTo {
                x: 250.0,
                y: 150.0,
                end_heading_radians: 0.0,
            },
        }))
        .unwrap();

        let (arrived, had_msg) = process_render_wake(&mut map, None, &rx, 0.016, false, true);

        assert!(arrived.is_empty());
        assert!(had_msg);
        let cursor = &map.cursors["default"].core;
        let replacement_path_len = cursor.path.as_ref().unwrap().length.max(1.0);
        assert!(cursor.dist > 0.0);
        assert!(cursor.dist < replacement_path_len);
    }

    #[test]
    fn idle_heartbeat_starts_fade_then_frames_return_to_quiescence() {
        let mut map = default_render_map();
        {
            let cursor = map.cursors.get_mut("default").unwrap();
            cursor.core.pos = (100.0, 100.0);
            cursor.core.motion.idle_hide_ms = 500.0;
            cursor.core.visual.reduced_motion = cursor_overlay::ReducedMotion::On;

            // Cheap 80 ms z-order heartbeats advance the idle clock without
            // requesting expensive frame paints during the opaque delay.
            for _ in 0..6 {
                cursor.tick(0.08);
                assert!(!cursor.needs_frame_tick());
                assert_eq!(cursor.core.idle_alpha, 1.0);
            }
        }

        // Shorten the final maintenance wait to the exact fade deadline rather
        // than overshooting by another 80 ms and jumping the first alpha frame.
        let deadline_wait = render_map_idle_wait_interval(&map).unwrap();
        assert!(deadline_wait >= Duration::from_millis(19));
        assert!(deadline_wait <= Duration::from_millis(21));

        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.tick(deadline_wait.as_secs_f64());
        assert!(cursor.needs_frame_tick());
        assert_eq!(cursor.core.idle_alpha, 1.0);

        cursor.tick(1.0 / 60.0);
        assert!(cursor.core.idle_alpha < 1.0);

        for _ in 0..60 {
            cursor.tick(1.0 / 60.0);
            if !cursor.needs_frame_tick() {
                break;
            }
        }

        assert!(!cursor.needs_frame_tick());
        assert_eq!(cursor.core.idle_alpha, 0.0);
        assert!(!render_map_needs_frame_tick(&map));
        assert!(!render_map_needs_z_order_tick(&map));
    }

    #[test]
    fn completed_click_pulse_returns_to_quiescence() {
        let mut map = default_render_map();
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.motion.idle_hide_ms = 0.0;
        cursor.core.visual.reduced_motion = cursor_overlay::ReducedMotion::On;
        cursor.apply_command(OverlayCommand::ClickPulse { x: 20.0, y: 30.0 });
        assert!(cursor.needs_frame_tick());

        for _ in 0..600 {
            cursor.tick(1.0 / 60.0);
            if !cursor.needs_frame_tick() {
                break;
            }
        }

        assert!(!cursor.needs_frame_tick());
        assert!(!render_map_needs_frame_tick(&map));
    }

    #[test]
    fn active_cursor_rendering_is_bounded_by_tile_not_root_size() {
        let mut map = default_render_map();
        map.scr_w = 7680;
        map.scr_h = 2160;
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.pos = (4000.0, 1000.0);

        let tiles = render_x11_tiles(&map);

        assert_eq!(tiles.len(), 1);
        let tile = &tiles[0];
        assert_eq!(tile.bounds.width, 128);
        assert_eq!(tile.bounds.height, 128);
        assert_eq!(tile.pixmap.data().len(), 128 * 128 * 4);
        assert!(tile.pixmap.data().len() < (map.scr_w * map.scr_h * 4) as usize);
    }

    #[test]
    fn session_badge_expands_only_the_local_cursor_tile() {
        let mut map = default_render_map();
        map.scr_w = 7680;
        map.scr_h = 2160;
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.pos = (4000.0, 1000.0);
        cursor.apply_command(OverlayCommand::SetSessionLabel("research-run".to_owned()));

        let tiles = render_x11_tiles(&map);

        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].bounds.width, 208);
        assert_eq!(tiles[0].bounds.height, 128);
        assert!(tiles[0].pixmap.data().len() < (map.scr_w * map.scr_h * 4) as usize);
    }

    #[test]
    fn modifier_only_badge_expands_the_local_cursor_tile() {
        let mut map = default_render_map();
        map.scr_w = 7680;
        map.scr_h = 2160;
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.pos = (4000.0, 1000.0);
        cursor.apply_command(OverlayCommand::BeginAction {
            action: CursorAction::Click,
            delivery: Some(cursor_overlay::DeliveryModifier::Foreground),
            target: Some(cursor_overlay::TargetModifier::Pixel),
        });

        let tiles = render_x11_tiles(&map);

        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].bounds.width, 208);
        assert_eq!(tiles[0].bounds.height, 128);
    }

    #[test]
    fn cursor_tiles_clip_at_screen_edges_and_skip_hidden_cursors() {
        let mut map = default_render_map();
        map.scr_w = 1920;
        map.scr_h = 1080;
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.pos = (10.0, 12.0);

        let tiles = render_x11_tiles(&map);
        assert_eq!(tiles.len(), 1);
        assert_eq!(
            tiles[0].bounds,
            X11TileBounds {
                x: 0,
                y: 0,
                width: 74,
                height: 76,
            }
        );

        map.cursors.get_mut("default").unwrap().core.visible = false;
        assert!(render_x11_tiles(&map).is_empty());
    }

    #[test]
    fn distant_cursors_use_independent_small_tiles() {
        let mut map = default_render_map();
        map.scr_w = 7680;
        map.scr_h = 2160;
        map.cursors.get_mut("default").unwrap().core.pos = (100.0, 100.0);
        let mut other = render_state_for_key(&map.template, "other");
        other.core.pos = (7400.0, 1800.0);
        map.cursors.insert("other".to_owned(), other);

        let tiles = render_x11_tiles(&map);

        assert_eq!(tiles.len(), 2);
        assert!(tiles
            .iter()
            .all(|tile| tile.bounds.width <= 128 && tile.bounds.height <= 128));
        assert_eq!(
            tiles
                .iter()
                .map(|tile| tile.pixmap.data().len())
                .sum::<usize>(),
            2 * 128 * 128 * 4
        );
    }

    #[test]
    fn visible_shape_contains_only_nontransparent_runs() {
        let mut pixmap = tiny_skia::Pixmap::new(4, 2).unwrap();
        pixmap.data_mut().copy_from_slice(&[
            1, 2, 3, 0, 10, 20, 30, 255, 11, 21, 31, 128, 4, 5, 6, 0, 7, 8, 9, 64, 1, 1, 1, 0, 2,
            2, 2, 0, 12, 22, 32, 255,
        ]);

        let (bgra, rectangles) = bgra_and_visible_shape(&pixmap, true);

        assert_eq!(&bgra[4..8], &[30, 20, 10, 255]);
        assert_eq!(rectangles.len(), 3);
        assert_eq!(
            (rectangles[0].x, rectangles[0].y, rectangles[0].width),
            (1, 0, 2)
        );
        assert_eq!(
            (rectangles[1].x, rectangles[1].y, rectangles[1].width),
            (0, 1, 1)
        );
        assert_eq!(
            (rectangles[2].x, rectangles[2].y, rectangles[2].width),
            (3, 1, 1)
        );
    }

    /// Golden for the documented fallback: whenever the real backdrop cannot be
    /// read (or a compositor is present), tiles still paint exactly like this.
    #[test]
    fn compositorless_shape_drops_bloom_and_unpremultiplies_visible_pixels() {
        let mut pixmap = tiny_skia::Pixmap::new(4, 1).unwrap();
        pixmap.data_mut().copy_from_slice(&[
            25, 12, 6, 127, 64, 32, 16, 128, 30, 20, 10, 180, 120, 80, 40, 255,
        ]);

        let (bgra, rectangles) = bgra_and_visible_shape(&pixmap, false);

        assert_eq!(&bgra[0..4], &[6, 12, 25, 127]);
        assert_eq!(&bgra[4..8], &[32, 64, 128, 255]);
        assert_eq!(&bgra[8..12], &[14, 28, 43, 255]);
        assert_eq!(&bgra[12..16], &[40, 80, 120, 255]);
        assert_eq!(rectangles.len(), 1);
        assert_eq!(
            (rectangles[0].x, rectangles[0].y, rectangles[0].width),
            (1, 0, 3)
        );
    }

    // ── Save-unders / software compositing ───────────────────────────────

    /// 4×1 premultiplied cursor strip with alphas 0 / 64 / 180 / 255.
    fn composite_test_pixmap() -> tiny_skia::Pixmap {
        let mut pixmap = tiny_skia::Pixmap::new(4, 1).unwrap();
        pixmap.data_mut().copy_from_slice(&[
            0, 0, 0, 0, // transparent
            32, 16, 8, 64, // faint bloom
            180, 90, 45, 180, // translucent body
            200, 100, 50, 255, // opaque core
        ]);
        pixmap
    }

    /// Matching BGRX root read: four distinct opaque desktop pixels.
    fn composite_test_backdrop() -> Vec<u8> {
        vec![
            10, 20, 30, 255, //
            40, 50, 60, 255, //
            70, 80, 90, 255, //
            100, 110, 120, 255,
        ]
    }

    fn tile_bounds(x: i16, y: i16, width: u16, height: u16) -> X11TileBounds {
        X11TileBounds {
            x,
            y,
            width,
            height,
        }
    }

    fn painted_rect(x: i16, y: i16, width: u16, height: u16) -> x11rb::protocol::xproto::Rectangle {
        x11rb::protocol::xproto::Rectangle {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn composited_tile_blends_translucent_pixels_over_the_backdrop() {
        let pixmap = composite_test_pixmap();
        let backdrop = composite_test_backdrop();

        let (bgra, _) = composited_bgra_and_visible_shape(&pixmap, &backdrop).unwrap();

        assert_eq!(&bgra[0..4], &[10, 20, 30, 255]);
        assert_eq!(&bgra[4..8], &[38, 53, 77, 255]);
        assert_eq!(&bgra[8..12], &[66, 114, 206, 255]);
        assert_eq!(&bgra[12..16], &[50, 100, 200, 255]);
        // Every uploaded pixel is opaque: the server has nothing left to blend.
        assert!(bgra.chunks_exact(4).all(|pixel| pixel[3] == 255));
    }

    #[test]
    fn composited_tile_reproduces_the_backdrop_where_the_cursor_is_transparent() {
        let pixmap = composite_test_pixmap();
        let backdrop = composite_test_backdrop();

        let (bgra, _) = composited_bgra_and_visible_shape(&pixmap, &backdrop).unwrap();

        assert_eq!(&bgra[0..3], &backdrop[0..3]);
    }

    #[test]
    fn composited_tile_output_depends_on_the_backdrop() {
        let pixmap = composite_test_pixmap();

        let (over_white, _) = composited_bgra_and_visible_shape(&pixmap, &[0xFF; 16]).unwrap();
        let (over_black, _) = composited_bgra_and_visible_shape(&pixmap, &[0x00; 16]).unwrap();

        assert_ne!(&over_white[4..8], &over_black[4..8]);
        assert_ne!(&over_white[8..12], &over_black[8..12]);
        // The opaque core hides whatever is beneath it.
        assert_eq!(&over_white[12..16], &over_black[12..16]);
    }

    #[test]
    fn composited_tile_keeps_the_bloom_the_cutoff_path_drops() {
        let pixmap = composite_test_pixmap();
        let backdrop = composite_test_backdrop();

        let (_, cutoff_shape) = bgra_and_visible_shape(&pixmap, false);
        let (bgra, shape) = composited_bgra_and_visible_shape(&pixmap, &backdrop).unwrap();

        // The alpha=64 bloom pixel sits at x=1: the cutoff path shapes it away.
        assert!(cutoff_shape.iter().all(|rect| rect.x > 1));
        assert!(shape
            .iter()
            .any(|rect| rect.x <= 1 && i32::from(rect.x) + i32::from(rect.width) > 1));
        // ...and its colour lands strictly between the backdrop and the
        // unpremultiplied source instead of being forced to either end.
        let unpremultiplied = [8u32 * 255 / 64, 16 * 255 / 64, 32 * 255 / 64];
        for (channel, source) in unpremultiplied.iter().enumerate() {
            let out = u32::from(bgra[4 + channel]);
            let back = u32::from(backdrop[4 + channel]);
            assert!(
                out > back.min(*source) && out < back.max(*source),
                "channel {channel}: {out} is not between {back} and {source}"
            );
        }
    }

    #[test]
    fn composited_tile_shape_matches_the_compositing_path() {
        let pixmap = composite_test_pixmap();
        let backdrop = composite_test_backdrop();

        let (_, compositing_shape) = bgra_and_visible_shape(&pixmap, true);
        let (_, composited_shape) = composited_bgra_and_visible_shape(&pixmap, &backdrop).unwrap();

        let as_tuples = |rects: Vec<x11rb::protocol::xproto::Rectangle>| {
            rects
                .into_iter()
                .map(|rect| (rect.x, rect.y, rect.width, rect.height))
                .collect::<Vec<_>>()
        };
        assert_eq!(as_tuples(compositing_shape), as_tuples(composited_shape));
    }

    #[test]
    fn composited_tile_rejects_a_mismatched_backdrop() {
        let pixmap = composite_test_pixmap();

        assert!(composited_bgra_and_visible_shape(&pixmap, &[0u8; 12]).is_none());
        assert!(composited_bgra_and_visible_shape(&pixmap, &[0u8; 20]).is_none());
    }

    /// A save-under as the paint path records one: the desktop that was under
    /// the rect, and the pixels the overlay put on top of it.
    fn saved_backdrop(
        bounds: X11TileBounds,
        under: Vec<u8>,
        uploaded: Vec<u8>,
    ) -> X11SavedBackdrop {
        X11SavedBackdrop {
            bounds,
            under,
            uploaded,
        }
    }

    #[test]
    fn saved_backdrop_serves_pixels_the_overlay_painted_last_frame() {
        let tile = tile_bounds(0, 0, 4, 1);
        let desktop: Vec<u8> = (0u8..16).collect();
        let mut cache = X11BackdropCache::default();
        let now = Instant::now();
        cache.record_frame(
            now,
            vec![painted_rect(0, 0, 2, 1)],
            vec![saved_backdrop(tile, desktop.clone(), vec![0xAA; 16])],
        );

        // 0xAA is what the overlay uploaded, so a root read that still returns
        // it is our own paint rather than desktop.
        let out = cache.resolve_backdrop(now, tile, &[0xAA; 16]).unwrap();

        assert_eq!(&out[0..8], &desktop[0..8]);
        assert_eq!(&out[8..16], &[0xAA; 8]);
    }

    #[test]
    fn a_repainted_pixel_is_read_live_even_inside_the_painted_rect() {
        let tile = tile_bounds(0, 0, 4, 1);
        let mut cache = X11BackdropCache::default();
        let now = Instant::now();
        cache.record_frame(
            now,
            vec![painted_rect(0, 0, 2, 1)],
            vec![saved_backdrop(tile, vec![0x11; 16], vec![0xAA; 16])],
        );

        // The owner answered its Expose: the first pixel no longer holds what
        // we uploaded, so the live read is the truth and the stale save-under
        // must not be served.
        let mut live = vec![0xAAu8; 16];
        live[0..4].copy_from_slice(&[0x77, 0x77, 0x77, 0xFF]);
        let out = cache.resolve_backdrop(now, tile, &live).unwrap();

        assert_eq!(&out[0..4], &[0x77, 0x77, 0x77, 0xFF]);
        assert_eq!(&out[4..8], &[0x11; 4]);
    }

    /// The halo regression: on a server whose root reads return black inside
    /// our own footprint (xorgxrdp + ARGB32 overlay), the mismatch must serve
    /// the save-under, not adopt the unreadable black — compositing each frame
    /// over the last otherwise drives a translucent shadow to solid black.
    #[test]
    fn untrusted_readback_serves_the_save_under_on_mismatch() {
        let tile = tile_bounds(0, 0, 4, 1);
        let mut cache = X11BackdropCache::default();
        cache.readback_untrusted = true;
        let now = Instant::now();
        cache.record_frame(
            now,
            vec![painted_rect(0, 0, 2, 1)],
            vec![saved_backdrop(tile, vec![0x11; 16], vec![0xAA; 16])],
        );

        // Live read is black where we painted — unreadable, not repainted.
        let mut live = vec![0xAAu8; 16];
        live[0..8].fill(0x00);
        let out = cache.resolve_backdrop(now, tile, &live).unwrap();

        // Both painted pixels come from the save-under; the unpainted rest of
        // the tile keeps the live read.
        assert_eq!(&out[0..8], &[0x11; 8]);
        assert_eq!(&out[8..16], &[0xAA; 8]);
    }

    #[test]
    fn readback_comparison_ignores_the_unused_fourth_byte() {
        let tile = tile_bounds(0, 0, 4, 1);
        let mut cache = X11BackdropCache::default();
        let now = Instant::now();
        // The overlay uploads alpha=255; a depth-24 root read pads with zero.
        let uploaded: Vec<u8> = (0..4).flat_map(|_| [0x20, 0x30, 0x40, 0xFF]).collect();
        let live: Vec<u8> = (0..4).flat_map(|_| [0x20, 0x30, 0x40, 0x00]).collect();
        cache.record_frame(
            now,
            vec![painted_rect(0, 0, 4, 1)],
            vec![saved_backdrop(tile, vec![0x11; 16], uploaded)],
        );

        assert_eq!(
            cache.resolve_backdrop(now, tile, &live).unwrap(),
            vec![0x11; 16]
        );
    }

    #[test]
    fn moving_cursor_reads_only_newly_exposed_pixels_from_the_root() {
        let first = tile_bounds(0, 0, 4, 1);
        let second = tile_bounds(2, 0, 4, 1);
        let mut cache = X11BackdropCache::default();
        let now = Instant::now();

        // Frame 1: empty cache, the whole tile is read live.
        let desktop: Vec<u8> = (1u8..17).collect();
        assert_eq!(
            cache.resolve_backdrop(now, first, &desktop).unwrap(),
            desktop
        );
        cache.record_frame(
            now,
            vec![painted_rect(1, 0, 2, 1)],
            vec![saved_backdrop(first, desktop.clone(), vec![0xAA; 16])],
        );

        // Frame 2: the shifted tile. 0xAA marks everything the root read would
        // hand back, including our own frame-1 pixels.
        let out = cache.resolve_backdrop(now, second, &[0xAA; 16]).unwrap();

        // x=2 was inside frame 1's shape and still holds our pixels → served
        // from the save-under.
        assert_eq!(&out[0..4], &desktop[8..12]);
        // x=3 was inside frame 1's tile but outside its shape → read live.
        assert_eq!(&out[4..8], &[0xAA; 4]);
        // x=4,5 are the newly exposed band → read live.
        assert_eq!(&out[8..16], &[0xAA; 8]);
    }

    #[test]
    fn badge_expansion_reads_the_new_side_strips_from_the_root() {
        let narrow = tile_bounds(136, 0, 128, 1);
        let widened = tile_bounds(96, 0, 208, 1);
        let mut cache = X11BackdropCache::default();
        let now = Instant::now();
        cache.record_frame(
            now,
            vec![painted_rect(136, 0, 128, 1)],
            vec![saved_backdrop(
                narrow,
                vec![0x33; 128 * 4],
                vec![0xAA; 128 * 4],
            )],
        );

        let out = cache
            .resolve_backdrop(now, widened, &vec![0xAA; 208 * 4])
            .unwrap();

        assert_eq!(&out[0..40 * 4], &vec![0xAA; 40 * 4][..]);
        assert_eq!(&out[40 * 4..168 * 4], &vec![0x33; 128 * 4][..]);
        assert_eq!(&out[168 * 4..208 * 4], &vec![0xAA; 40 * 4][..]);
    }

    #[test]
    fn resolving_fails_when_a_painted_pixel_has_no_saved_backdrop() {
        let tile = tile_bounds(0, 0, 4, 1);
        let mut cache = X11BackdropCache::default();
        let now = Instant::now();
        cache.record_frame(now, vec![painted_rect(0, 0, 2, 1)], Vec::new());

        assert!(cache.resolve_backdrop(now, tile, &[0xAA; 16]).is_none());
    }

    #[test]
    fn a_frame_is_vacated_when_it_is_superseded_not_when_it_was_painted() {
        let tile = tile_bounds(0, 0, 4, 1);
        let elsewhere = tile_bounds(8, 0, 4, 1);
        let start = Instant::now();
        let mut cache = X11BackdropCache::default();
        cache.record_frame(
            start,
            vec![painted_rect(0, 0, 2, 1)],
            vec![saved_backdrop(tile, vec![0x11; 16], vec![0xAA; 16])],
        );

        // The cursor rests for five seconds, then moves. The old frame's pixels
        // were vacated just now, so the window under them has not repainted yet
        // and the save-under has to survive the next few frames.
        let moved = start + Duration::from_secs(5);
        for step in 0..3 {
            cache.record_frame(
                moved + Duration::from_millis(16 * step),
                vec![painted_rect(8, 0, 2, 1)],
                vec![saved_backdrop(elsewhere, vec![0x22; 16], vec![0xAA; 16])],
            );
        }

        let out = cache
            .resolve_backdrop(moved + Duration::from_millis(48), tile, &[0xAA; 16])
            .unwrap();
        assert_eq!(&out[0..8], &[0x11; 8]);
    }

    #[test]
    fn a_long_vacated_frame_stops_being_consulted_without_waiting_for_a_prune() {
        let tile = tile_bounds(0, 0, 4, 1);
        let elsewhere = tile_bounds(8, 0, 4, 1);
        let start = Instant::now();
        let mut cache = X11BackdropCache::default();
        cache.record_frame(
            start,
            vec![painted_rect(0, 0, 2, 1)],
            vec![saved_backdrop(tile, vec![0x11; 16], vec![0xAA; 16])],
        );
        cache.record_frame(
            start + Duration::from_millis(16),
            vec![painted_rect(8, 0, 2, 1)],
            vec![saved_backdrop(elsewhere, vec![0x22; 16], vec![0xAA; 16])],
        );

        // Nothing painted for a long while, so nothing pruned either: expiry is
        // applied where the save-unders are consumed.
        let later = start + X11_BACKDROP_RETENTION + Duration::from_secs(1);
        assert_eq!(
            cache.resolve_backdrop(later, tile, &[0xAA; 16]).unwrap(),
            vec![0xAA; 16]
        );
    }

    #[test]
    fn pruning_keeps_the_frame_that_is_still_on_screen() {
        let start = Instant::now();
        let mut visible = X11BackdropCache::default();
        visible.record_frame(
            start,
            vec![painted_rect(0, 0, 2, 1)],
            vec![saved_backdrop(
                tile_bounds(0, 0, 4, 1),
                vec![0x11; 16],
                vec![0xAA; 16],
            )],
        );
        visible.prune(start + Duration::from_secs(10));
        assert_eq!(visible.frames.len(), 1);

        let mut cleared = X11BackdropCache::default();
        cleared.record_frame(start, Vec::new(), Vec::new());
        cleared.prune(start + Duration::from_secs(10));
        assert!(cleared.frames.is_empty());
    }

    #[test]
    fn a_clearing_paint_vacates_the_frame_it_replaces_without_dropping_it() {
        let tile = tile_bounds(0, 0, 4, 1);
        let start = Instant::now();
        let mut cache = X11BackdropCache::default();
        cache.record_frame(
            start,
            vec![painted_rect(0, 0, 2, 1)],
            vec![saved_backdrop(tile, vec![0x11; 16], vec![0xAA; 16])],
        );
        // The cursor faded out: the tile is cleared, but the trail it leaves is
        // still on screen until its owner repaints it.
        cache.record_frame(start + Duration::from_millis(16), Vec::new(), Vec::new());

        let out = cache
            .resolve_backdrop(start + Duration::from_millis(32), tile, &[0xAA; 16])
            .unwrap();
        assert_eq!(&out[0..8], &[0x11; 8]);
    }

    #[test]
    fn retained_frames_are_capped() {
        let start = Instant::now();
        let mut cache = X11BackdropCache::default();
        for step in 0..(X11_BACKDROP_MAX_FRAMES + 8) {
            cache.record_frame(
                start + Duration::from_millis(16 * step as u64),
                vec![painted_rect(0, 0, 2, 1)],
                vec![saved_backdrop(
                    tile_bounds(0, 0, 4, 1),
                    vec![0x11; 16],
                    vec![0xAA; 16],
                )],
            );
        }

        assert_eq!(cache.frames.len(), X11_BACKDROP_MAX_FRAMES);
    }

    #[test]
    fn invalidate_drops_every_saved_backdrop() {
        let tile = tile_bounds(0, 0, 4, 1);
        let mut cache = X11BackdropCache::default();
        let now = Instant::now();
        cache.record_frame(
            now,
            vec![painted_rect(0, 0, 4, 1)],
            vec![saved_backdrop(tile, vec![0x11; 16], vec![0xAA; 16])],
        );

        cache.invalidate();

        assert_eq!(
            cache.resolve_backdrop(now, tile, &[0xAA; 16]).unwrap(),
            vec![0xAA; 16]
        );
    }

    #[test]
    fn a_resync_blanks_once_and_suppresses_painting_for_the_grace() {
        let tile = tile_bounds(0, 0, 4, 1);
        let start = Instant::now();
        let mut cache = X11BackdropCache::default();
        cache.record_frame(
            start,
            vec![painted_rect(0, 0, 4, 1)],
            vec![saved_backdrop(tile, vec![0x11; 16], vec![0xAA; 16])],
        );

        cache.resync(start);

        // The save-unders are gone and painting is suppressed, so no root read
        // can happen while our own pixels are still on screen.
        assert!(cache.frames.is_empty());
        assert!(cache.resyncing(start));
        assert!(cache.take_resync_blanking());
        assert!(!cache.take_resync_blanking());
        assert!(cache.resyncing(start + X11_BACKDROP_RESYNC_GRACE - Duration::from_millis(1)));
        assert!(!cache.resyncing(start + X11_BACKDROP_RESYNC_GRACE));
        // The next resync blanks again.
        cache.resync(start + X11_BACKDROP_RESYNC_GRACE);
        assert!(cache.take_resync_blanking());
    }

    #[test]
    fn a_transient_backdrop_failure_defers_frames_before_latching_compositing_off() {
        let mut cache = X11BackdropCache::default();

        for _ in 1..X11_BACKDROP_UNBACKED_FRAME_LIMIT {
            assert!(!cache.note_unbacked_frame());
            assert!(cache.enabled());
        }
        // A frame that composites clears the streak, so a one-off X11 error
        // cannot cost the session its translucency.
        cache.note_composited_frame();
        for _ in 1..X11_BACKDROP_UNBACKED_FRAME_LIMIT {
            assert!(!cache.note_unbacked_frame());
        }
        assert!(cache.enabled());

        assert!(cache.note_unbacked_frame());
        assert!(!cache.enabled());
    }

    #[test]
    fn slow_backdrop_reads_latch_compositing_off() {
        let over_budget = X11_BACKDROP_READ_BUDGET + Duration::from_millis(1);
        let mut cache = X11BackdropCache::default();

        cache.note_read_duration(over_budget);
        cache.note_read_duration(over_budget);
        assert!(cache.enabled());

        // A fast frame clears the streak.
        cache.note_read_duration(Duration::from_millis(1));
        cache.note_read_duration(over_budget);
        cache.note_read_duration(over_budget);
        assert!(cache.enabled());

        cache.note_read_duration(over_budget);
        assert!(!cache.enabled());
        assert!(cache
            .resolve_backdrop(Instant::now(), tile_bounds(0, 0, 4, 1), &[0xAA; 16])
            .is_none());
    }

    #[test]
    fn backdrop_compositing_is_disabled_under_a_compositor() {
        let fresh = X11BackdropCache::default();
        let latched_off = X11BackdropCache {
            disabled: true,
            ..X11BackdropCache::default()
        };

        assert!(!backdrop_compositing_enabled(true, &fresh));
        assert!(!backdrop_compositing_enabled(true, &latched_off));
        assert!(!backdrop_compositing_enabled(false, &latched_off));
        assert!(backdrop_compositing_enabled(false, &fresh));
    }

    #[test]
    fn a_save_under_keeps_only_the_painted_bounding_box() {
        let shape = vec![painted_rect(2, 1, 3, 1), painted_rect(1, 2, 2, 2)];

        let bbox = shape_bounding_box(&shape).unwrap();

        assert_eq!(bbox, (1, 1, 5, 4));
        assert!(shape_bounding_box(&[]).is_none());
    }

    #[test]
    fn cropping_a_tile_buffer_keeps_row_order() {
        // 3×3 tile, one distinct byte quad per pixel.
        let tile: Vec<u8> = (0u8..9)
            .flat_map(|index| [index, index, index, 255])
            .collect();

        let cropped = crop_tile_pixels(&tile, 3, (1, 1, 3, 3));

        assert_eq!(
            cropped,
            vec![4, 4, 4, 255, 5, 5, 5, 255, 7, 7, 7, 255, 8, 8, 8, 255]
        );
    }

    // ── Framebuffer model ────────────────────────────────────────────────
    //
    // The save-under logic is only correct against a real screen, where our own
    // paint is physically present in the pixels a root read returns. These
    // tests model that screen: paint writes the overlay's pixels into it, reads
    // come back out of it, and the windows underneath answer their Expose after
    // a configurable delay. The invariant every frame asserts is the one the
    // whole design exists for — the backdrop we composite over is the real
    // desktop, never our own previous frame.

    /// 8×1 premultiplied cursor strip: transparent edges, a translucent bloom
    /// and an opaque core, i.e. every alpha class the shape has to handle.
    fn strip_cursor_pixmap() -> tiny_skia::Pixmap {
        let mut pixmap = tiny_skia::Pixmap::new(8, 1).unwrap();
        let alphas = [0u8, 40, 120, 255, 255, 120, 40, 0];
        let data = pixmap.data_mut();
        for (index, alpha) in alphas.into_iter().enumerate() {
            let scale = |value: u32| ((value * u32::from(alpha)) / 255) as u8;
            data[index * 4..index * 4 + 4].copy_from_slice(&[
                scale(200),
                scale(120),
                scale(60),
                alpha,
            ]);
        }
        pixmap
    }

    struct ScreenModel {
        /// What the windows underneath would draw: the answer every composited
        /// backdrop has to reproduce.
        desktop: Vec<u8>,
        /// What a root read actually returns, including our own paint.
        screen: Vec<u8>,
        painted_at: Vec<Option<usize>>,
        /// Frames a vacated pixel takes to be repainted by its owner; `None`
        /// models a client that never answers its Expose at all.
        repaint_delay: Option<usize>,
    }

    impl ScreenModel {
        fn new(width: usize, repaint_delay: Option<usize>) -> Self {
            let desktop: Vec<u8> = (0..width)
                .flat_map(|x| [x as u8, (x * 3) as u8, (x * 7) as u8, 0])
                .collect();
            Self {
                screen: desktop.clone(),
                painted_at: vec![None; width],
                desktop,
                repaint_delay,
            }
        }

        fn read(&self, x: usize, width: usize) -> Vec<u8> {
            self.screen[x * 4..(x + width) * 4].to_vec()
        }

        fn desktop_slice(&self, x: usize, width: usize) -> Vec<u8> {
            self.desktop[x * 4..(x + width) * 4].to_vec()
        }

        /// Redraw part of the desktop. Pixels the overlay is currently covering
        /// keep our paint: the server clips their owner away from them.
        fn redraw_desktop(&mut self, range: std::ops::Range<usize>, tint: u8) {
            for pixel in range {
                let at = pixel * 4;
                self.desktop[at..at + 4].copy_from_slice(&[tint, tint / 2, tint / 3, 0]);
                if self.painted_at[pixel].is_none() {
                    self.screen[at..at + 4].copy_from_slice(&self.desktop[at..at + 4]);
                }
            }
        }

        fn paint(
            &mut self,
            frame: usize,
            x: usize,
            bgra: &[u8],
            shape: &[x11rb::protocol::xproto::Rectangle],
        ) {
            for rect in shape {
                for col in usize::from(rect.x as u16)..usize::from(rect.x as u16 + rect.width) {
                    let at = (x + col) * 4;
                    // The server keeps our colour bytes; a depth-24 root read
                    // pads the fourth byte with zero rather than our 255.
                    self.screen[at..at + 3].copy_from_slice(&bgra[col * 4..col * 4 + 3]);
                    self.screen[at + 3] = 0;
                    self.painted_at[x + col] = Some(frame);
                }
            }

            let Some(delay) = self.repaint_delay else {
                return;
            };
            for pixel in 0..self.painted_at.len() {
                let Some(at_frame) = self.painted_at[pixel] else {
                    continue;
                };
                if at_frame < frame && frame - at_frame > delay {
                    let at = pixel * 4;
                    self.screen[at..at + 4].copy_from_slice(&self.desktop[at..at + 4]);
                    self.painted_at[pixel] = None;
                }
            }
        }
    }

    /// Drive the real cache and the real compositing helper across `path`,
    /// asserting on every frame that the backdrop resolves to the true desktop.
    fn run_screen_model(model: &mut ScreenModel, path: &[usize], redraw: Option<(usize, u8)>) {
        let pixmap = strip_cursor_pixmap();
        let start = Instant::now();
        let mut cache = X11BackdropCache::default();

        for (frame, &x) in path.iter().enumerate() {
            if let Some((at_frame, tint)) = redraw {
                if frame == at_frame {
                    model.redraw_desktop(0..6, tint);
                }
            }
            let now = start + Duration::from_millis(16 * frame as u64);
            let bounds = tile_bounds(x as i16, 0, 8, 1);

            let fresh = model.read(x, 8);
            let under = cache
                .resolve_backdrop(now, bounds, &fresh)
                .unwrap_or_else(|| panic!("frame {frame}: no backdrop"));
            assert_eq!(
                under,
                model.desktop_slice(x, 8),
                "frame {frame} composited over something other than the desktop"
            );

            let (bgra, mut shape) = composited_bgra_and_visible_shape(&pixmap, &under).unwrap();
            model.paint(frame, x, &bgra, &shape);

            let saved: Vec<X11SavedBackdrop> = shape_bounding_box(&shape)
                .map(|bbox| X11SavedBackdrop {
                    bounds: tile_bounds(
                        (x + bbox.0) as i16,
                        0,
                        (bbox.2 - bbox.0) as u16,
                        (bbox.3 - bbox.1) as u16,
                    ),
                    under: crop_tile_pixels(&under, 8, bbox),
                    uploaded: crop_tile_pixels(&bgra, 8, bbox),
                })
                .into_iter()
                .collect();
            for rect in &mut shape {
                rect.x = rect.x.saturating_add(x as i16);
            }
            cache.record_frame(now, shape, saved);
        }
    }

    /// One pass right and back again, so every frame's tile overlaps the trail
    /// the previous frames left behind.
    fn sweep_path(limit: usize) -> Vec<usize> {
        (0..=limit).chain((0..limit).rev()).collect()
    }

    #[test]
    fn a_moving_cursor_never_composites_over_its_own_previous_frame() {
        for delay in [Some(0), Some(1), Some(5)] {
            let mut model = ScreenModel::new(64, delay);
            run_screen_model(&mut model, &sweep_path(12), None);
        }
    }

    #[test]
    fn a_client_that_never_answers_expose_still_gets_a_clean_backdrop() {
        // The 200 ms-style timing guess is exactly what this case used to
        // break: nothing under the trail is ever repainted, so every vacated
        // pixel still holds our own paint when it is read back.
        let mut model = ScreenModel::new(64, None);
        run_screen_model(&mut model, &sweep_path(12), None);
    }

    #[test]
    fn a_backdrop_that_changed_while_uncovered_is_picked_up_again() {
        // The trail is repainted quickly and then its owner draws something
        // else there. Returning over it must show the new content, not the
        // save-under captured on the way out.
        let mut model = ScreenModel::new(64, Some(1));
        run_screen_model(&mut model, &sweep_path(12), Some((13, 0x5A)));
    }
}

#[cfg(not(target_os = "linux"))]
fn run_overlay_thread(_cfg: CursorConfig, _rx: std::sync::mpsc::Receiver<OverlayMsg>) {}
