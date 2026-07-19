//! Linux agent-cursor overlay — X11 RGBA override-redirect window.
//!
//! Architecture:
//! - Creates an override-redirect (non-reparented) X11 window with 32-bit ARGB visual
//!   from XComposite.  The window covers the full display area.
//! - A background thread renders cursor-local tiles at ~60 Hz while animation
//!   is active and uploads them with XPutImage + XShape clipping.
//! - XShape clips both input and visible pixels. On bare X11, the visible shape
//!   quantizes the rendered alpha mask so translucent bloom pixels do not turn
//!   into an opaque black disk when no compositor is present.
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

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
const X11_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[cfg(target_os = "linux")]
use cursor_overlay::ZOrderEnforcer;
use cursor_overlay::{
    CursorConfig, CursorKey, KeyedOverlayCommand, OverlayCommand, OverlayMsg, Palette,
    RenderStateCore,
};

// ── Global channel ────────────────────────────────────────────────────────

static CMD_TX: OnceLock<std::sync::mpsc::SyncSender<OverlayMsg>> = OnceLock::new();
static CMD_RX_CELL: Mutex<Option<std::sync::mpsc::Receiver<OverlayMsg>>> = Mutex::new(None);
static RENDER: Mutex<Option<RenderMap>> = Mutex::new(None);
static ARRIVAL_TX: Mutex<Option<HashMap<CursorKey, tokio::sync::oneshot::Sender<()>>>> =
    Mutex::new(None);

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
    disable_render_state: bool,
}

#[cfg(target_os = "linux")]
impl Drop for X11OverlayThreadCleanup {
    fn drop(&mut self) {
        // Dropping every sender releases active animate_cursor_to waiters. On
        // X11, also make future calls observe the renderer as unavailable. A
        // Wayland session may still use its native forwarding path even when
        // the optional XWayland owner thread cannot start.
        release_all_arrivals();
        if self.disable_render_state {
            if let Ok(mut guard) = RENDER.lock() {
                disable_render_map(&mut guard);
            }
        }
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
    let mut rs = RenderState::new(template.clone());
    rs.core.palette = Palette::for_instance(key);
    rs
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
        OverlayMsg::Cmd(KeyedOverlayCommand { key, cmd }) => {
            if map.ended.contains(&key) {
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
    let (tx, rx) = std::sync::mpsc::sync_channel(4096);
    let _ = CMD_TX.set(tx);
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
    // Also forward to the native-Wayland layer-shell overlay when Wayland
    // is opted in. The wayland overlay's `forward` is a no-op when its
    // owner thread isn't started yet (which is the normal X11-only case).
    #[cfg(target_os = "linux")]
    {
        if crate::wayland::is_wayland() {
            if crate::wayland::shell_helper::available() {
                // GNOME has no layer-shell. Drive only the final positioning
                // commands through the compositor helper; it performs its own
                // easing and avoids starting a worker that must fail.
                match &cmd {
                    cursor_overlay::OverlayCommand::ClickPulse { x, y } => {
                        crate::wayland::shell_helper::click_pulse(*x as i32, *y as i32);
                    }
                    cursor_overlay::OverlayCommand::SnapTo { x, y, .. } => {
                        crate::wayland::shell_helper::move_cursor(*x as i32, *y as i32);
                    }
                    _ => {}
                }
            } else {
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
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(OverlayMsg::Remove(key));
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
    error.error_kind == x11rb::protocol::ErrorKind::Window
        && error.major_opcode == x11rb::protocol::xproto::CONFIGURE_WINDOW_REQUEST
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
        // The pinned target can disappear after the synchronous liveness probe
        // in X11ZOrderEnforcer::reassert but before its unchecked ConfigureWindow
        // request reaches the server. x11rb then delivers BadWindow here after
        // the VoidCookie is dropped. This owner connection's only other
        // ConfigureWindow path is checked synchronously, so a non-overlay bad
        // window is the stale sibling and is safe to retry without a sibling on
        // the next eligible z-order reassertion.
        x11rb::protocol::Event::Error(error)
            if recoverable_x11_z_order_error(error, overlay_win) =>
        {
            tracing::debug!(
                stale_target = error.bad_value,
                "X11 overlay target disappeared during z-order reassertion"
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
    for shape_kind in [SK::BOUNDING, SK::INPUT] {
        conn.shape_rectangles(SO::SET, shape_kind, ClipOrdering::UNSORTED, win, 0, 0, &[])?
            .check()?;
    }
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

// ── X11 thread ────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn run_overlay_thread(cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayMsg>) {
    use x11rb::connection::Connection;
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::ConnectionExt as XprotoConnectionExt;
    use x11rb::protocol::xproto::{
        AtomEnum, ColormapAlloc, CreateGCAux, CreateWindowAux, EventMask, PropMode, WindowClass,
    };
    use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

    let _cleanup = X11OverlayThreadCleanup {
        disable_render_state: !crate::wayland::is_wayland(),
    };

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
    for shape_kind in [SK::INPUT, SK::BOUNDING] {
        conn.shape_rectangles(
            SO::SET,
            shape_kind,
            x11rb::protocol::xproto::ClipOrdering::UNSORTED,
            win,
            0,
            0,
            &[],
        )
        .ok();
    }

    conn.map_window(win).ok();
    conn.flush().ok();

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
    let z_enforcer = X11ZOrderEnforcer { conn: &conn, win };

    loop {
        // Idle fast path: no Pixmap allocation, RGBA→BGRA copy, XShape update,
        // or XPutImage until pixels can change. The 50 ms maintenance bound
        // services X11/RandR events; a resting visible cursor also reasserts
        // z-order at most every 80 ms. Neither maintenance path authorizes paint.
        let (first_msg, maintenance_timeout) =
            match wait_for_overlay_work(&rx, frame_tick_needed, maintenance_deadline) {
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
            compositor_present = x11_compositor_present(&conn, screen_num);
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

        let now = Instant::now();
        let elapsed_dt = now.duration_since(last_tick).as_secs_f64();
        last_tick = now;

        // Drain commands and tick.
        let (
            arrived,
            pinned_wid,
            had_msg,
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
                    &rx,
                    elapsed_dt,
                    maintenance_timeout,
                    frame_tick_needed,
                );
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
                    next_frame_tick_needed,
                    next_z_order_tick_needed,
                    next_idle_wait_interval,
                )
            } else {
                break;
            }
        };

        // Reassert immediately after a command or target change, then at most
        // every 80 ms while any cursor remains visible. Maintenance wakes keep
        // that stacking contract without authorizing a repaint.
        let pin_changed = pinned_wid != last_pinned;
        last_pinned = pinned_wid;
        if had_msg || screen_changed || pin_changed || last_ztick.elapsed() >= z_order_interval {
            last_ztick = Instant::now();
            z_enforcer.reassert(pinned_wid);
        }

        // Render after a command, during an active animation/fade, and once
        // more as the final active state settles. This leaves the X11 window in
        // its completed/cleared state before the next blocking receive.
        if had_msg || screen_changed || frame_tick_needed || next_frame_tick_needed {
            let tiles = {
                let guard = RENDER.lock().unwrap();
                guard.as_ref().map(render_x11_tiles)
            };

            if let Some(tiles) = tiles {
                if let Err(e) = paint_x11_tiles(
                    &conn,
                    win,
                    depth,
                    gc_id,
                    &tiles,
                    compositor_present,
                    screen_changed,
                ) {
                    // A broken X11 connection cannot recover inside this
                    // owner thread. Exit instead of spinning at frame cadence
                    // and repeatedly allocating/rendering work nobody can see.
                    tracing::warn!("X11 overlay paint failed; disabling overlay: {e}");
                    break;
                }
            }
        }

        // Preserve the original ordering: callers waiting on arrival only
        // resume after the destination frame has reached the X11 window.
        for key in &arrived {
            arrival_fire(key);
        }

        frame_tick_needed = next_frame_tick_needed;
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
        // intended z-reassertion. Probe liveness via get_window_attributes
        // before committing to the sibling path.
        let target_live = target.and_then(|xid| {
            self.conn
                .get_window_attributes(xid as u32)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|_| xid)
        });
        let aux = if let Some(target_xid) = target_live {
            // Place overlay just above the pinned X11 window.
            ConfigureWindowAux::new()
                .sibling(target_xid as u32)
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
    let left = (core.pos.0 - X11_CURSOR_TILE_MARGIN).floor() as i32;
    let top = (core.pos.1 - X11_CURSOR_TILE_MARGIN).floor() as i32;
    let right = (core.pos.0 + X11_CURSOR_TILE_MARGIN).ceil() as i32;
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
            // composite pixels regardless of upload order.
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

#[cfg(target_os = "linux")]
fn paint_x11_tiles(
    conn: &impl x11rb::connection::Connection,
    win: u32,
    depth: u8,
    gc_id: u32,
    tiles: &[X11PaintTile],
    compositor_present: bool,
    checked_requests: bool,
) -> anyhow::Result<()> {
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::{ConnectionExt as XprotoConnectionExt, ImageFormat};

    let mut payloads = Vec::with_capacity(tiles.len());
    let mut visible_shape = Vec::new();
    for tile in tiles {
        let (bgra, mut tile_shape) = bgra_and_visible_shape(&tile.pixmap, compositor_present);
        for rect in &mut tile_shape {
            rect.x = rect.x.saturating_add(tile.bounds.x);
            rect.y = rect.y.saturating_add(tile.bounds.y);
        }
        visible_shape.extend(tile_shape);
        payloads.push((tile.bounds, bgra));
    }

    for (bounds, bgra) in payloads {
        let cookie = conn.put_image(
            ImageFormat::Z_PIXMAP,
            win,
            gc_id,
            bounds.width,
            bounds.height,
            bounds.x,
            bounds.y,
            0,
            depth,
            &bgra,
        )?;
        if checked_requests {
            cookie.check()?;
        }
    }

    // Keep the previous (or fail-closed empty) shape while uploading pixels,
    // then expose only the freshly rendered alpha runs. This ordering prevents
    // stale/zero-filled backing pixels from becoming visible during repair.
    let shape_cookie = conn.shape_rectangles(
        SO::SET,
        SK::BOUNDING,
        x11rb::protocol::xproto::ClipOrdering::UNSORTED,
        win,
        0,
        0,
        &visible_shape,
    )?;
    if checked_requests {
        shape_cookie.check()?;
    }

    conn.flush()?;
    Ok(())
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
    fn non_badwindow_configure_error_remains_fatal() {
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

    #[test]
    fn resting_visible_cursor_only_requires_cheap_z_order_ticks() {
        let mut map = default_render_map();
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.pos = (100.0, 100.0);
        cursor.core.motion.idle_hide_ms = 0.0;

        assert!(!render_map_needs_frame_tick(&map));
        assert!(render_map_needs_z_order_tick(&map));
    }

    #[test]
    fn disabling_settled_cursor_clears_once_then_parks() {
        let mut map = default_render_map();
        let cursor = map.cursors.get_mut("default").unwrap();
        cursor.core.pos = (100.0, 100.0);
        cursor.core.motion.idle_hide_ms = 0.0;
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
                    cmd: OverlayCommand::SetPalette(Palette::for_instance("other")),
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
}

#[cfg(not(target_os = "linux"))]
fn run_overlay_thread(_cfg: CursorConfig, _rx: std::sync::mpsc::Receiver<OverlayMsg>) {}
