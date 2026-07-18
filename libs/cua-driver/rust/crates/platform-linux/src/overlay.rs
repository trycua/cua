//! Linux agent-cursor overlay — X11 RGBA override-redirect window.
//!
//! Architecture:
//! - Creates an override-redirect (non-reparented) X11 window with 32-bit ARGB visual
//!   from XComposite.  The window covers the full display area.
//! - A background thread renders frames at ~60 Hz using tiny-skia and XShmPutImage
//!   (or XPutImage fallback) with XRender ARGB compositing.
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
    if key.is_empty() {
        return;
    }
    let msg = OverlayMsg::Cmd(KeyedOverlayCommand {
        key: key.clone(),
        cmd: cmd.clone(),
    });
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(msg.clone());
    }
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

    send_command_for(
        key,
        OverlayCommand::MoveTo {
            x,
            y,
            end_heading_radians: std::f64::consts::FRAC_PI_4,
        },
    );

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
    /// quiescent, so an idle MCP server can block on the command channel
    /// instead of repainting a full-screen X11 pixmap at 60 fps.
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
    idle_wait_interval: Option<Duration>,
) -> Instant {
    let z_order_deadline = last_z_order_tick + z_order_interval;
    idle_wait_interval
        .map(|interval| (last_tick + interval).min(z_order_deadline))
        .unwrap_or(z_order_deadline)
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
    z_order_tick_needed: bool,
    maintenance_deadline: Instant,
) -> OverlayWake {
    if frame_tick_needed {
        return OverlayWake::Frame;
    }

    if z_order_tick_needed {
        let timeout = maintenance_deadline.saturating_duration_since(Instant::now());
        return match rx.recv_timeout(timeout) {
            Ok(msg) => OverlayWake::Message(msg),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => OverlayWake::MaintenanceTimeout,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => OverlayWake::Disconnected,
        };
    }

    match rx.recv() {
        Ok(msg) => OverlayWake::Message(msg),
        Err(_) => OverlayWake::Disconnected,
    }
}

// ── X11 thread ────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn run_overlay_thread(cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayMsg>) {
    use x11rb::connection::Connection;
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::ConnectionExt as XprotoConnectionExt;
    use x11rb::protocol::xproto::{
        AtomEnum, ColormapAlloc, CreateWindowAux, EventMask, PropMode, WindowClass,
    };
    use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

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
    let scr_w = screen.width_in_pixels as u32;
    let scr_h = screen.height_in_pixels as u32;
    let compositor_present = x11_compositor_present(&conn, screen_num);
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

    // Main render loop at ~60 Hz while pixels can change. When every cursor is
    // quiescent, block on the command channel and leave the last frame intact.
    let frame_dur = Duration::from_millis(16);
    let z_order_interval = Duration::from_millis(80);
    let mut last_tick = Instant::now();
    let mut last_ztick = Instant::now();
    let mut frame_tick_needed = false;
    let mut z_order_tick_needed = false;
    let mut maintenance_deadline = last_ztick + z_order_interval;
    let mut last_pinned: Option<u64> = None;
    let z_enforcer = X11ZOrderEnforcer { conn: &conn, win };

    loop {
        // Idle fast path: no full-screen Pixmap allocation, RGBA→BGRA copy,
        // XShape update, or XPutImage until a command arrives. A resting visible
        // cursor still wakes cheaply every 80 ms to preserve the documented
        // X11 z-order contract without repainting.
        let (first_msg, maintenance_timeout) = match wait_for_overlay_work(
            &rx,
            frame_tick_needed,
            z_order_tick_needed,
            maintenance_deadline,
        ) {
            OverlayWake::Frame => (None, false),
            OverlayWake::Message(msg) => (Some(msg), false),
            OverlayWake::MaintenanceTimeout => (None, true),
            OverlayWake::Disconnected => break,
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
        if had_msg || pin_changed || last_ztick.elapsed() >= z_order_interval {
            last_ztick = Instant::now();
            z_enforcer.reassert(pinned_wid);
        }

        // Render after a command, during an active animation/fade, and once
        // more as the final active state settles. This leaves the X11 window in
        // its completed/cleared state before the next blocking receive.
        if had_msg || frame_tick_needed || next_frame_tick_needed {
            let pixmap = {
                let guard = RENDER.lock().unwrap();
                guard.as_ref().map(|map| {
                    let mut pm = tiny_skia::Pixmap::new(map.scr_w.max(1), map.scr_h.max(1))
                        .unwrap_or_else(|| tiny_skia::Pixmap::new(1, 1).unwrap());
                    for rs in map.cursors.values() {
                        // TODO: thread X11/Wayland scale-factor here so cursors
                        // render at native resolution on HiDPI Linux displays
                        // (`XRRGetCrtcInfo` reports per-output DPI; the GNOME
                        // scale-factor setting is exposed via the screen-scaling
                        // protocol). For now we default to 1.0 — preserves
                        // pre-retina-fix behaviour on Linux.
                        cursor_overlay::paint_cursor(&mut pm, &rs.core, 0.0, 0.0, None, 1.0);
                    }
                    pm
                })
            };

            if let Some(pm) = pixmap {
                paint_x11(
                    &conn,
                    win,
                    scr_w,
                    scr_h,
                    depth,
                    visual_id,
                    &pm,
                    compositor_present,
                );
            }
        }

        // Preserve the original ordering: callers waiting on arrival only
        // resume after the destination frame has reached the X11 window.
        for key in &arrived {
            arrival_fire(key);
        }

        // Drain any X events (needed to avoid blocking).
        while let Ok(Some(_)) = conn.poll_for_event() {}

        frame_tick_needed = next_frame_tick_needed;
        z_order_tick_needed = next_z_order_tick_needed;
        maintenance_deadline = next_maintenance_deadline(
            last_tick,
            last_ztick,
            z_order_interval,
            next_idle_wait_interval,
        );
        if frame_tick_needed {
            let elapsed = Instant::now().duration_since(last_tick);
            if let Some(remaining) = frame_dur.checked_sub(elapsed) {
                std::thread::sleep(remaining);
            }
        }
    }
}

// ── Z-order enforcer (Linux impl of cursor_overlay::ZOrderEnforcer) ──────

/// X11 implementation of [`cursor_overlay::ZOrderEnforcer`].
///
/// Borrows the X11 connection and overlay window id; lives only inside
/// `run_overlay_thread` (the X11 connection is not `'static`). Called
/// every 80 ms from the render loop.
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

/// Blit a tiny-skia pixmap to the X11 window using XPutImage (ZPixmap).
/// The pixmap is premultiplied RGBA; X11 ARGB is premultiplied BGRA.
#[cfg(target_os = "linux")]
const NONCOMPOSITED_ALPHA_CUTOFF: u8 = 128;

#[cfg(target_os = "linux")]
fn paint_x11(
    conn: &impl x11rb::connection::Connection,
    win: u32,
    w: u32,
    h: u32,
    depth: u8,
    _visual_id: u32,
    pm: &tiny_skia::Pixmap,
    compositor_present: bool,
) {
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::{ConnectionExt as XprotoConnectionExt, CreateGCAux, ImageFormat};
    if pm.width() == 0 || pm.height() == 0 {
        return;
    }

    // Create a GC for the window if we don't have one.
    // (Simplified: we recreate it every frame which is safe but not optimal.)
    let gc_id = match conn.generate_id() {
        Ok(id) => id,
        Err(_) => return,
    };
    let gc_aux = CreateGCAux::new();
    if conn.create_gc(gc_id, win, &gc_aux).is_err() {
        return;
    }

    let (bgra, visible_shape) = bgra_and_visible_shape(pm, compositor_present);

    // A 32-bit ARGB window needs a compositor to blend transparent pixels.
    // Clip the native window to the rendered alpha runs; the converter below
    // quantizes those runs when no compositor owns the screen selection.
    let _ = conn.shape_rectangles(
        SO::SET,
        SK::BOUNDING,
        x11rb::protocol::xproto::ClipOrdering::UNSORTED,
        win,
        0,
        0,
        &visible_shape,
    );

    // XPutImage (ZPixmap).
    let _ = conn.put_image(
        ImageFormat::Z_PIXMAP,
        win,
        gc_id,
        w as u16,
        h as u16,
        0,
        0,
        0,
        depth,
        &bgra,
    );

    conn.free_gc(gc_id).ok();
    conn.flush().ok();
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
    fn active_frame_wake_does_not_consume_queued_command() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(test_message()).unwrap();

        assert!(matches!(
            wait_for_overlay_work(&rx, true, false, Instant::now()),
            OverlayWake::Frame
        ));
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn idle_wait_wakes_for_command() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(test_message()).unwrap();

        match wait_for_overlay_work(&rx, false, false, Instant::now()) {
            OverlayWake::Message(OverlayMsg::Cmd(keyed)) => assert_eq!(keyed.key, "default"),
            _ => panic!("idle wait did not return the queued command"),
        }
    }

    #[test]
    fn resting_cursor_wait_times_out_for_maintenance() {
        let (_tx, rx) = std::sync::mpsc::channel();
        assert!(matches!(
            wait_for_overlay_work(&rx, false, true, Instant::now() + Duration::from_millis(1)),
            OverlayWake::MaintenanceTimeout
        ));
    }

    #[test]
    fn expired_maintenance_deadline_times_out_immediately() {
        let (_tx, rx) = std::sync::mpsc::channel();
        let deadline = Instant::now() - Duration::from_millis(1);
        assert!(matches!(
            wait_for_overlay_work(&rx, false, true, deadline),
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
            Some(Duration::from_millis(20)),
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
    fn idle_wait_reports_disconnected_sender_in_both_modes() {
        let (tx, rx) = std::sync::mpsc::channel();
        drop(tx);
        assert!(matches!(
            wait_for_overlay_work(&rx, false, false, Instant::now()),
            OverlayWake::Disconnected
        ));

        let (tx, rx) = std::sync::mpsc::channel();
        drop(tx);
        assert!(matches!(
            wait_for_overlay_work(&rx, false, true, Instant::now()),
            OverlayWake::Disconnected
        ));
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
