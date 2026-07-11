//! macOS agent-cursor overlay — transparent click-through NSWindow.
//!
//! ## Architecture
//!
//! The MCP/tokio server runs on a **background thread** (spawned in
//! `cua-driver/src/main.rs`).  AppKit MUST run on the **main thread**.
//! The two sides communicate through a global lock-free channel:
//!
//! - MCP tool calls → `send_command(OverlayCommand)` → `CMD_TX` (SyncSender)
//! - main thread → `run_on_main_thread()` → drains `CMD_RX` every frame
//!
//! The render loop uses a background thread at the display refresh cadence only
//! while pixels can change. Each cursor owns a small nonactivating AppKit panel:
//! position changes move the panel, and only appearance changes redraw the
//! small layer bitmap.
//!
//! ## Coordinate system
//!
//! All coordinates are **screen points** with the **top-left origin**
//! (matching `OverlayCommand::MoveTo` and AX element coordinates).  The
//! Each cursor panel is positioned in AppKit's bottom-left screen coordinates,
//! while tiny-skia still paints in the top-left cursor coordinate space.
//!
//! ## Cross-platform note (2026-05 dedup audit)
//!
//! Animation state + render pipeline live in `cursor_overlay::render_state`
//! (`RenderStateCore`, `tick_swift_constants`, `apply_command_base`,
//! `render_frame`).  macOS uses the hardcoded Swift reference constants
//! (peakSpeed=900, springK=400, overshoot=0.8) and the sentinel-snap
//! variants of MoveTo / ClickPulse — see the wrapper around
//! `apply_command_base` below.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cursor_overlay::{
    CursorConfig, CursorKey, FocusRect, KeyedOverlayCommand, MotionConfig, OverlayCommand,
    OverlayMsg, Palette, RenderStateCore, ZOrderEnforcer,
};
use indexmap::IndexMap;

// ── Arrival-signal channels (one waiter slot per cursor key) ──────────────
//
// Each session's `animate_cursor_to` registers an arrival oneshot keyed by its
// own cursor key. A new animation only supersedes the SAME key's prior waiter,
// so concurrent sessions never cross-cancel each other's arrivals.

struct ArrivalWaiter {
    generation: u64,
    tx: tokio::sync::oneshot::Sender<()>,
}

static ARRIVAL_TX: Mutex<Option<HashMap<CursorKey, ArrivalWaiter>>> = Mutex::new(None);
static NEXT_ARRIVAL_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

const ARRIVAL_TIMEOUT: Duration = Duration::from_secs(30);

fn arrival_register(key: CursorKey, tx: tokio::sync::oneshot::Sender<()>) -> u64 {
    let generation = NEXT_ARRIVAL_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut guard = ARRIVAL_TX.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    // Cancel only the same key's previous waiter (superseded by new animation).
    if let Some(old) = map.insert(key, ArrivalWaiter { generation, tx }) {
        let _ = old.tx.send(());
    }
    generation
}

fn arrival_fire(key: &CursorKey) {
    if let Ok(mut guard) = ARRIVAL_TX.lock() {
        if let Some(map) = guard.as_mut() {
            if let Some(waiter) = map.remove(key) {
                let _ = waiter.tx.send(());
            }
        }
    }
}

fn arrival_cancel(key: &CursorKey, generation: u64) {
    if let Ok(mut guard) = ARRIVAL_TX.lock() {
        if let Some(map) = guard.as_mut() {
            if map
                .get(key)
                .is_some_and(|waiter| waiter.generation == generation)
            {
                map.remove(key);
            }
        }
    }
}

async fn wait_for_arrival(
    key: &CursorKey,
    generation: u64,
    rx: tokio::sync::oneshot::Receiver<()>,
    timeout: Duration,
) -> bool {
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) | Err(_) => {
            arrival_cancel(key, generation);
            false
        }
    }
}

// ── Global overlay state ──────────────────────────────────────────────────

static CMD_TX: OnceLock<std::sync::mpsc::Sender<OverlayMsg>> = OnceLock::new();
static PENDING_COMMANDS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
const MAX_PENDING_COMMANDS: usize = 4096;
// Single-consumer slot; receiver is moved into run_on_main_thread().
static CMD_RX_CELL: Mutex<Option<std::sync::mpsc::Receiver<OverlayMsg>>> = Mutex::new(None);
static RENDER: Mutex<Option<RenderMap>> = Mutex::new(None);

/// The keyed, insertion-ordered collection of owned cursors that the render
/// loop composites every frame. Insertion order = stable z-order (later keys
/// paint on top). `win_w` / `win_h` are screen-global, hoisted out of the
/// per-cursor `RenderState` (written once in `run_appkit`).
struct RenderMap {
    cursors: IndexMap<CursorKey, RenderState>,
    /// Most recent cursor key touched by an inbound command. Idle-dwell
    /// timeout selection prefers this cursor so a stale visible cursor cannot
    /// shorten the dwell of the cursor the user just moved.
    last_active_key: Option<CursorKey>,
    win_w: f64,
    win_h: f64,
    /// Frozen launch-time config used as the template for lazily-created
    /// cursors (its palette is overridden per-key via `Palette::for_instance`).
    template: CursorConfig,
    /// Render-side tombstone of ended session cursor keys. A `Cmd`
    /// for a key in here is dropped WITHOUT get-or-create, so an in-flight
    /// click/move from another task that lands AFTER the owning session's
    /// `Remove` can never resurrect the just-removed cursor (the ghost-cursor
    /// resurrection race). An explicitly ordered `Revive` clears the matching
    /// key when `start_session` intentionally reuses an id. "default" is never
    /// tombstoned.
    ended: std::collections::HashSet<CursorKey>,
}

/// Build the `RenderState` for a lazily-created cursor key: derive from the
/// launch template but give each non-default key its own palette so distinct
/// sessions get distinct colours automatically.
fn render_state_for_key(template: &CursorConfig, key: &str) -> RenderState {
    let mut rs = RenderState::new(template.clone());
    rs.core.palette = Palette::for_instance(key);
    rs
}

/// Apply one inbound [`OverlayMsg`] to the render map (drain step). Factored
/// out as a pure function so the per-session ownership + removal lifecycle is
/// unit-testable without AppKit.
///
/// Returns the resolved cursor key for a `Cmd` (so the caller can track the
/// last-active key for z-order pinning); `None` for a `Remove`.
fn apply_msg(map: &mut RenderMap, msg: OverlayMsg) -> Option<CursorKey> {
    match msg {
        OverlayMsg::Remove(key) => {
            // The "default" cursor backs the anonymous / one-shot path and
            // must survive every session_end + the daemon lifetime.
            if key != "default" {
                map.cursors.shift_remove(&key);
                if map.last_active_key.as_deref() == Some(key.as_str()) {
                    map.last_active_key = None;
                }
                if let Ok(mut guard) = ARRIVAL_TX.lock() {
                    if let Some(m) = guard.as_mut() {
                        m.remove(&key);
                    }
                }
                // Tombstone the key so a late in-flight Cmd from another task
                // (an animate/click racing the owning session's death) cannot
                // re-create the just-removed cursor. Never tombstone "default".
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
            // Drop a command for an already-ended session WITHOUT get-or-create
            // — this is the resurrection guard. Without it, a ClickPulse/MoveTo
            // landing after Remove would re-insert (and re-leak) the cursor.
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
            map.last_active_key = Some(k.clone());
            Some(k)
        }
    }
}

/// Initialise global overlay state (call once, before run_on_main_thread).
pub fn init(cfg: CursorConfig) {
    static REVIVE_HOOK_REGISTERED: OnceLock<()> = OnceLock::new();
    REVIVE_HOOK_REGISTERED.get_or_init(|| {
        cua_driver_core::session::register_session_revive_hook(|session_id| {
            revive_cursor(session_id.to_owned());
        });
    });
    // Commands and lifecycle events share one ordered, non-blocking producer
    // queue. A stalled renderer can build a backlog, but it cannot block
    // session end/revival or make those lifecycle events disappear.
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = CMD_TX.set(tx);
    PENDING_COMMANDS.store(0, std::sync::atomic::Ordering::Release);
    *CMD_RX_CELL.lock().unwrap() = Some(rx);
    *ARRIVAL_TX.lock().unwrap() = Some(HashMap::new());
    let mut cursors = IndexMap::new();
    cursors.insert("default".to_owned(), RenderState::new(cfg.clone()));
    *RENDER.lock().unwrap() = Some(RenderMap {
        cursors,
        last_active_key: None,
        win_w: 0.0,
        win_h: 0.0,
        template: cfg,
        ended: std::collections::HashSet::new(),
    });
}

/// Send a keyed command from any thread (MCP tool, etc.). Non-blocking except
/// for allocation; failure means the renderer receiver has stopped.
pub fn send_command(key: CursorKey, cmd: OverlayCommand) -> bool {
    // Empty key is the explicit no-cursor sentinel → drop the command so a
    // cursor-less run never paints.
    if key.is_empty() {
        return false;
    }
    try_send_command(CMD_TX.get(), key, cmd)
}

fn try_send_command(
    sender: Option<&std::sync::mpsc::Sender<OverlayMsg>>,
    key: CursorKey,
    cmd: OverlayCommand,
) -> bool {
    try_send_command_with_depth(sender, key, cmd, &PENDING_COMMANDS)
}

fn try_send_command_with_depth(
    sender: Option<&std::sync::mpsc::Sender<OverlayMsg>>,
    key: CursorKey,
    cmd: OverlayCommand,
    pending: &std::sync::atomic::AtomicUsize,
) -> bool {
    let reserved = pending
        .fetch_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |count| (count < MAX_PENDING_COMMANDS).then_some(count + 1),
        )
        .is_ok();
    if !reserved {
        return false;
    }
    let sent = sender.is_some_and(|tx| {
        tx.send(OverlayMsg::Cmd(KeyedOverlayCommand { key, cmd }))
            .is_ok()
    });
    if !sent {
        pending.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
    sent
}

fn note_command_dequeued(msg: &OverlayMsg) {
    if matches!(msg, OverlayMsg::Cmd(_)) {
        PENDING_COMMANDS.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

fn send_registered_move(
    sender: Option<&std::sync::mpsc::Sender<OverlayMsg>>,
    key: CursorKey,
    cmd: OverlayCommand,
    generation: u64,
) -> bool {
    if try_send_command(sender, key.clone(), cmd) {
        true
    } else {
        arrival_cancel(&key, generation);
        false
    }
}

/// Convenience for callsites not yet threaded with a session key: drives the
/// seeded `"default"` cursor (the anonymous / one-shot identity).
pub fn send_command_default(cmd: OverlayCommand) {
    let _ = send_command("default".to_owned(), cmd);
}

/// Remove a session's owned cursor from the render collection (fired from the
/// `session_end` hook). The `"default"` key is guarded against removal on the
/// render side, so this is a no-op for it; removing an absent key (anonymous
/// session that never created a cursor) is a harmless no-op.
pub fn remove_cursor(key: CursorKey) {
    if key.is_empty() {
        return;
    }
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.send(OverlayMsg::Remove(key));
    }
}

/// Clear an ended session's render-side tombstone after an explicit
/// `start_session` revival. It travels over the same channel as `Remove` and
/// commands, so a late command queued before revival remains rejected while a
/// command queued after revival is accepted.
pub fn revive_cursor(key: CursorKey) {
    if key.is_empty() || key == "default" {
        return;
    }
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.send(OverlayMsg::Revive(key));
    }
}

/// Return a snapshot of a cursor's current motion config (for use by
/// set_agent_cursor_motion to apply partial overrides without losing other
/// knobs). Reads the motion of the cursor `key`, falling back to the
/// `"default"` cursor's motion when that key has no own entry yet (e.g. a
/// session whose first motion call precedes any move/enable).
pub fn current_motion(key: &str) -> MotionConfig {
    let guard = RENDER.lock().unwrap();
    let Some(map) = guard.as_ref() else {
        return MotionConfig::default();
    };
    map.cursors
        .get(key)
        .or_else(|| map.cursors.get("default"))
        .map(|rs| rs.core.motion.clone())
        .unwrap_or_default()
}

/// Seed a brand-new (sentinel-positioned) cursor at an on-screen start point
/// offset up-left of `(target_x, target_y)` so the immediately-following
/// `MoveTo` glides INTO the target instead of silently snapping. Without this,
/// a cursor's very first action (common on a pure-AX run — launch app, AX-press
/// a button) produces no visible motion: `animate_cursor_to` early-returned at
/// the sentinel and only `ClickPulse` snapped a static arrow, which is easy to
/// miss. See the AX-no-glide report.
///
/// No-op when the cursor is already on-screen (pos.0 > -50.0) or absent. The
/// seed is clamped to the main screen frame so it never starts off-display.
/// Returns true if a seed was applied (i.e. the cursor was at the sentinel and
/// is now primed to glide).
fn seed_start_if_sentinel(key: &CursorKey, target_x: f64, target_y: f64) -> bool {
    let mut guard = RENDER.lock().unwrap();
    let Some(map) = guard.as_mut() else {
        return false;
    };
    seed_start_in_map(map, key, target_x, target_y)
}

/// Pure seed step operating on a borrowed [`RenderMap`] — factored out of
/// `seed_start_if_sentinel` so the get-or-create + clamp logic is unit-testable
/// without the global `RENDER` static or AppKit.
fn seed_start_in_map(map: &mut RenderMap, key: &CursorKey, target_x: f64, target_y: f64) -> bool {
    // Offset the start up-left of the target so the Dubins path has room to
    // curve in; 140pt is enough to read as motion at 900pt/s peak speed.
    const SEED_OFFSET: f64 = 140.0;
    let (win_w, win_h) = (map.win_w, map.win_h);
    // Respect the resurrection guard: never seed (and thus re-create) a cursor
    // whose session already ended.
    if map.ended.contains(key) {
        return false;
    }
    // Get-or-create the cursor so the very first AX action seeds + glides even
    // when the lazy render-thread creation hasn't drained the PinAbove yet
    // (the render loop's drain would otherwise win the race and the seed read
    // an absent cursor). Mirrors apply_msg's entry().or_insert_with.
    let template = map.template.clone();
    let k = key.clone();
    let rs = map
        .cursors
        .entry(key.clone())
        .or_insert_with(|| render_state_for_key(&template, &k));
    if !(rs.core.cfg.enabled && rs.core.pos.0 < -50.0) {
        return false;
    }
    let mut sx = target_x - SEED_OFFSET;
    let mut sy = target_y - SEED_OFFSET;
    // Clamp into the screen frame when we know it (win_w/h are 0 until the
    // AppKit window is up; in that headless case the unclamped seed is still
    // on-screen-by-construction for any realistic target).
    if win_w > 0.0 && win_h > 0.0 {
        sx = sx.clamp(2.0, win_w - 2.0);
        sy = sy.clamp(2.0, win_h - 2.0);
        // If clamping collapsed the seed onto the target (target in a corner),
        // nudge it the other way so there is still a visible glide distance.
        if (sx - target_x).abs() < 8.0 && (sy - target_y).abs() < 8.0 {
            sx = (target_x + SEED_OFFSET).min(win_w - 2.0);
            sy = (target_y + SEED_OFFSET).min(win_h - 2.0);
        }
    }
    rs.core.pos = (sx, sy);
    rs.core.idle_secs = 0.0;
    rs.core.idle_alpha = 1.0;
    true
}

/// Animate the overlay cursor to `(x, y)` and suspend until the Dubins path
/// completes and the spring overshoot begins.
///
/// Mirrors Swift's `AgentCursor.shared.animateAndWait(to:)`.
/// Returns immediately (no animation) only when the overlay is disabled for
/// this cursor. A brand-new cursor still at the off-screen sentinel is first
/// seeded on-screen via [`seed_start_if_sentinel`] so its FIRST action glides
/// in (it previously snapped silently via `ClickPulse`, invisible on a pure-AX
/// run).
pub async fn animate_cursor_to(key: CursorKey, x: f64, y: f64) {
    // Empty key is the explicit no-cursor sentinel → nothing to animate.
    if key.is_empty() {
        return;
    }
    // Seed a sentinel cursor on-screen so the MoveTo below glides instead of
    // being short-circuited. After this the cursor's pos.0 > -50.0, so the
    // should-animate check passes on the first action just like later ones.
    seed_start_if_sentinel(&key, x, y);

    // Check whether animation should run for THIS cursor. A disabled cursor
    // never animates; an absent cursor (seed found nothing to prime) is skipped.
    let should_animate = {
        let guard = RENDER.lock().unwrap();
        match guard.as_ref().and_then(|m| m.cursors.get(&key)) {
            Some(rs) if rs.core.cfg.enabled && rs.core.pos.0 > -50.0 => true,
            _ => false,
        }
    };
    if !should_animate {
        return;
    }

    // Create a one-shot channel; store the sender (keyed) so the render thread
    // can fire it when this cursor's path finishes.
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let generation = arrival_register(key.clone(), tx);

    // Send the MoveTo command (click offset applied inside apply_command).
    let sent = send_registered_move(
        CMD_TX.get(),
        key.clone(),
        OverlayCommand::MoveTo {
            x,
            y,
            // Arrive pointing upper-left (45°), matching the macOS system-cursor
            // convention and Swift reference (`endAngleDegrees: 45`).
            end_heading_radians: std::f64::consts::FRAC_PI_4,
        },
        generation,
    );

    if !sent {
        return;
    }

    // Await arrival signal (fired from render thread when Dubins path ends),
    // but never let a stopped/stalled renderer hang a tool call indefinitely.
    let _ = wait_for_arrival(&key, generation, rx, ARRIVAL_TIMEOUT).await;
}

/// Block the calling thread (must be the OS main thread) running the AppKit
/// event loop and the overlay window.  Never returns normally.
///
/// Call this from `main()` after spawning the tokio background thread.
pub fn run_on_main_thread() {
    // Take the receiver.
    let rx = match CMD_RX_CELL.lock().unwrap().take() {
        Some(r) => r,
        None => {
            // init() was never called — no overlay, just spin.
            loop {
                std::thread::park();
            }
        }
    };

    let cfg = {
        let guard = RENDER.lock().unwrap();
        match guard.as_ref() {
            Some(m) => m.template.clone(),
            None => return,
        }
    };

    if !cfg.enabled {
        loop {
            std::thread::park();
        }
    }

    // AppKit's `+[NSApplication sharedApplication]` registers the process with
    // the Window Server and ABORTS the whole process (SIGABRT in
    // `_RegisterApplication`) when there's no graphic-session access — e.g.
    // `mcp` run as a stdio child from SSH, a LaunchDaemon, or headless CI.
    // Detect that without touching AppKit and run headless: the MCP server
    // keeps serving on its background thread while this thread just parks,
    // exactly as it does when the overlay is disabled. See issue #1724.
    if !crate::session::has_graphic_access() {
        tracing::warn!(
            "no Window Server / graphic-session access — skipping cursor \
             overlay and running headless (issue #1724)"
        );
        loop {
            std::thread::park();
        }
    }

    // ------------------------------------------------------------------
    // AppKit setup (all on the main thread).
    // ------------------------------------------------------------------
    unsafe { run_appkit(cfg, rx) };
}

// ── Animation / render state ──────────────────────────────────────────────
//
// The platform-agnostic fields + tick + apply_command + render pipeline live
// in `cursor_overlay::render_state` (2026-05 dedup audit). What stays here
// is the macOS-specific NSScreen window dimensions and the focus-rect
// overlay (a macOS-only post-arrival element highlight).

struct RenderState {
    core: RenderStateCore,
    /// Focus-highlight rectangle `[x, y, w, h]` in screen coords; None = not shown.
    focus_rect: Option<[f64; 4]>,
    /// Fade progress for the focus rect: 0.0 = fully visible, 1.0 = gone.
    focus_rect_t: f64,
}

impl RenderState {
    fn new(cfg: CursorConfig) -> Self {
        RenderState {
            core: RenderStateCore::new(cfg),
            focus_rect: None,
            focus_rect_t: 1.0,
        }
    }

    /// Advance the animation by `dt`.  Uses the Swift reference constants
    /// (peakSpeed=900, springK=400, overshoot=0.8) — see
    /// [`RenderStateCore::tick_swift_constants`].  Returns true if an
    /// arrival signal should be fired (the path just ended).
    fn tick(&mut self, dt: f64) -> bool {
        let fire_arrival = self.core.tick_swift_constants(dt);

        // Advance focus-rect fade (fades out over ~600ms).  macOS-only —
        // the shared core has no focus_rect concept.
        if self.focus_rect.is_some() {
            self.focus_rect_t = (self.focus_rect_t + dt / 0.6).min(1.0);
            if self.focus_rect_t >= 1.0 {
                self.focus_rect = None;
                self.focus_rect_t = 1.0;
            }
        }

        fire_arrival
    }

    fn apply_command(&mut self, cmd: OverlayCommand) {
        // macOS uses the sentinel-snap variants of MoveTo / ClickPulse:
        //   - MoveTo only snaps `self.pos` if the cursor is still at the
        //     off-screen sentinel `(-200, -200)` (otherwise the path starts
        //     from the current position so the animation is continuous).
        //   - ClickPulse only updates `self.pos` if the cursor is still at
        //     the sentinel (otherwise the animation already landed it there).
        match cmd {
            OverlayCommand::ShowFocusRect(rect) => {
                self.focus_rect = rect;
                self.focus_rect_t = 0.0; // reset fade to fully visible
            }
            other => {
                let _ = self.core.apply_command_base(other, true, true);
            }
        }
    }

    fn active_animation_or_focus(&self) -> bool {
        self.core.path.is_some()
            || self.core.spring.is_some()
            || self.core.click_t.is_some()
            || self.focus_rect.is_some()
    }

    fn visible_on_screen(&self) -> bool {
        self.core.visible && self.core.pos.0 >= -100.0
    }

    fn idle_fade_in_progress(&self) -> bool {
        self.core.motion.idle_hide_ms > 0.0
            && self.visible_on_screen()
            && self.core.idle_alpha < 1.0
            && self.core.idle_alpha >= 0.004
    }

    /// True while the render loop must wake at frame cadence because the next
    /// tick can change pixels. A fully visible idle-hide dwell is deliberately
    /// NOT a frame-tick state: pixels are static until fade_start, so the render
    /// loop can sleep on recv_timeout() instead of compositing at 60fps.
    fn needs_frame_tick(&self) -> bool {
        self.active_animation_or_focus() || self.idle_fade_in_progress()
    }

    /// Remaining dwell time before the idle-hide fade should begin. During
    /// this interval the last frame is already correct, so no composite is
    /// needed unless a command arrives first.
    fn idle_fade_due_in(&self) -> Option<Duration> {
        if self.active_animation_or_focus()
            || self.core.motion.idle_hide_ms <= 0.0
            || !self.visible_on_screen()
            || self.core.idle_alpha < 1.0
        {
            return None;
        }

        let fade_start = self.core.motion.idle_hide_ms / 1000.0;
        let remaining = (fade_start - self.core.idle_secs).max(0.0);
        Some(Duration::from_secs_f64(remaining))
    }
}

fn render_map_needs_frame_tick(map: &RenderMap) -> bool {
    map.cursors.values().any(RenderState::needs_frame_tick)
}

fn render_map_idle_fade_due_in(map: &RenderMap) -> Option<Duration> {
    if let Some(due) = map
        .last_active_key
        .as_ref()
        .and_then(|key| map.cursors.get(key))
        .and_then(RenderState::idle_fade_due_in)
    {
        return Some(due);
    }
    map.cursors
        .values()
        .filter_map(RenderState::idle_fade_due_in)
        .min()
}

fn frame_budget_for_max_fps(max_fps: i64) -> Duration {
    let fps = if max_fps >= 30 { max_fps } else { 60 };
    Duration::from_secs_f64(1.0 / fps as f64)
}

fn repin_frame_interval(frame_budget: Duration) -> u32 {
    let secs = frame_budget.as_secs_f64();
    if secs <= 0.0 {
        return 60;
    }
    (1.0 / secs).round().clamp(1.0, 240.0) as u32
}

const CURSOR_WINDOW_MARGIN_POINTS: f64 = 72.0;
const FOCUS_RECT_MARGIN_POINTS: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct LogicalRect {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

impl LogicalRect {
    fn around(point: (f64, f64), radius: f64) -> Self {
        Self {
            left: point.0 - radius,
            top: point.1 - radius,
            width: radius * 2.0,
            height: radius * 2.0,
        }
    }

    fn expanded(self, margin: f64) -> Self {
        Self {
            left: self.left - margin,
            top: self.top - margin,
            width: self.width + margin * 2.0,
            height: self.height + margin * 2.0,
        }
    }

    fn union(self, other: Self) -> Self {
        let right = (self.left + self.width).max(other.left + other.width);
        let bottom = (self.top + self.height).max(other.top + other.height);
        let left = self.left.min(other.left);
        let top = self.top.min(other.top);
        Self {
            left,
            top,
            width: right - left,
            height: bottom - top,
        }
    }

    fn pixel_size(self, backing_scale: f64) -> (u32, u32) {
        let scale = backing_scale.max(1.0);
        (
            (self.width * scale).ceil().max(1.0) as u32,
            (self.height * scale).ceil().max(1.0) as u32,
        )
    }

    fn contains(self, point: (f64, f64)) -> bool {
        point.0 >= self.left
            && point.0 < self.left + self.width
            && point.1 >= self.top
            && point.1 < self.top + self.height
    }

    fn intersection_area(self, other: Self) -> f64 {
        let left = self.left.max(other.left);
        let top = self.top.max(other.top);
        let right = (self.left + self.width).min(other.left + other.width);
        let bottom = (self.top + self.height).min(other.top + other.height);
        (right - left).max(0.0) * (bottom - top).max(0.0)
    }

    fn distance_squared_to(self, point: (f64, f64)) -> f64 {
        let right = self.left + self.width;
        let bottom = self.top + self.height;
        let dx = if point.0 < self.left {
            self.left - point.0
        } else if point.0 > right {
            point.0 - right
        } else {
            0.0
        };
        let dy = if point.1 < self.top {
            self.top - point.1
        } else if point.1 > bottom {
            point.1 - bottom
        } else {
            0.0
        };
        dx * dx + dy * dy
    }

    fn is_valid(self) -> bool {
        self.left.is_finite()
            && self.top.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
    }
}

#[derive(Debug, Clone, Copy)]
struct ScreenGeometry {
    origin_x: f64,
    origin_y: f64,
    height: f64,
    fallback_backing_scale: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DisplayGeometry {
    bounds: LogicalRect,
    backing_scale: f64,
}

#[derive(Debug, Clone, Copy)]
struct AppKitRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelLevel {
    Floating,
    TargetWindow,
}

impl PanelLevel {
    fn ns_window_level(self) -> i64 {
        match self {
            PanelLevel::Floating => 3,
            PanelLevel::TargetWindow => 0,
        }
    }
}

fn panel_level_for_pin(pinned_wid: Option<u64>) -> PanelLevel {
    if pinned_wid.is_some() {
        PanelLevel::TargetWindow
    } else {
        PanelLevel::Floating
    }
}

fn appkit_frame_for_rect(rect: LogicalRect, screen: ScreenGeometry) -> AppKitRect {
    AppKitRect {
        x: screen.origin_x + rect.left,
        y: screen.origin_y + screen.height - rect.top - rect.height,
        width: rect.width,
        height: rect.height,
    }
}

fn normalized_backing_scale(scale: f64, fallback: f64) -> f64 {
    let fallback = if fallback.is_finite() && fallback > 0.0 {
        fallback.max(1.0)
    } else {
        1.0
    };
    if scale.is_finite() && scale > 0.0 {
        scale.max(1.0)
    } else {
        fallback
    }
}

fn active_display_geometries(fallback_scale: f64) -> Vec<DisplayGeometry> {
    use core_graphics::display::CGDisplay;

    let Ok(display_ids) = CGDisplay::active_displays() else {
        return Vec::new();
    };
    let mut displays: Vec<DisplayGeometry> = Vec::with_capacity(display_ids.len());
    for display_id in display_ids {
        let bounds = CGDisplay::new(display_id).bounds();
        let logical = LogicalRect {
            left: bounds.origin.x,
            top: bounds.origin.y,
            width: bounds.size.width,
            height: bounds.size.height,
        };
        if !logical.is_valid() {
            continue;
        }
        let scale = normalized_backing_scale(
            crate::tools::get_screen_size::get_backing_scale(
                display_id,
                logical.width.round() as i64,
            ),
            fallback_scale,
        );

        // Mirrored displays may report identical logical bounds. A single
        // raster must satisfy both, so retain the denser backing scale.
        if let Some(existing) = displays
            .iter_mut()
            .find(|display| display.bounds == logical)
        {
            existing.backing_scale = existing.backing_scale.max(scale);
        } else {
            displays.push(DisplayGeometry {
                bounds: logical,
                backing_scale: scale,
            });
        }
    }
    displays
}

fn window_bounds_snapshot() -> HashMap<u64, LogicalRect> {
    crate::windows::all_windows()
        .into_iter()
        .filter_map(|window| {
            let bounds = LogicalRect {
                left: window.bounds.x,
                top: window.bounds.y,
                width: window.bounds.width,
                height: window.bounds.height,
            };
            bounds
                .is_valid()
                .then_some((window.window_id as u64, bounds))
        })
        .collect()
}

fn display_for_cursor<'a>(
    displays: &'a [DisplayGeometry],
    cursor_point: (f64, f64),
    pinned_target: Option<LogicalRect>,
) -> Option<&'a DisplayGeometry> {
    if let Some(target) = pinned_target.filter(|bounds| bounds.is_valid()) {
        let mut best: Option<(&DisplayGeometry, f64, bool)> = None;
        for display in displays {
            let area = display.bounds.intersection_area(target);
            if area <= 0.0 {
                continue;
            }
            let contains_cursor = display.bounds.contains(cursor_point);
            let replace = best.is_none_or(|(_, best_area, best_contains_cursor)| {
                area > best_area || (area == best_area && contains_cursor && !best_contains_cursor)
            });
            if replace {
                best = Some((display, area, contains_cursor));
            }
        }
        if let Some((display, _, _)) = best {
            return Some(display);
        }
    }

    if !cursor_point.0.is_finite() || !cursor_point.1.is_finite() {
        return None;
    }
    displays
        .iter()
        .find(|display| display.bounds.contains(cursor_point))
        .or_else(|| {
            displays.iter().min_by(|left, right| {
                left.bounds
                    .distance_squared_to(cursor_point)
                    .total_cmp(&right.bounds.distance_squared_to(cursor_point))
            })
        })
}

fn backing_scale_for_cursor(
    displays: &[DisplayGeometry],
    fallback_scale: f64,
    cursor_point: (f64, f64),
    pinned_target: Option<LogicalRect>,
) -> f64 {
    display_for_cursor(displays, cursor_point, pinned_target)
        .map(|display| normalized_backing_scale(display.backing_scale, fallback_scale))
        .unwrap_or_else(|| normalized_backing_scale(fallback_scale, 1.0))
}

fn backing_scale_changed(previous: Option<f64>, next: f64) -> bool {
    previous.is_none_or(|previous| (previous - next).abs() > 0.001)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AppearanceSignature {
    heading_millirad: i64,
    idle_alpha_milli: i64,
    pressed: bool,
    click_milli: i64,
    focus_milli: i64,
    builtin_shape: u8,
    custom_shape: Option<(u32, u32)>,
}

fn quantize(value: f64, scale: f64) -> i64 {
    (value * scale).round() as i64
}

fn builtin_shape_code(core: &RenderStateCore) -> u8 {
    match core.cfg.builtin_shape {
        cursor_overlay::BuiltinShape::Arrow => 0,
        cursor_overlay::BuiltinShape::Teardrop => 1,
        cursor_overlay::BuiltinShape::Sky => 2,
    }
}

fn active_shape_rotates_with_heading(rs: &RenderState) -> bool {
    match rs.core.shape.as_ref() {
        Some(shape) => shape.rotates_with_heading,
        None => !matches!(rs.core.cfg.builtin_shape, cursor_overlay::BuiltinShape::Sky),
    }
}

fn appearance_signature(rs: &RenderState) -> AppearanceSignature {
    AppearanceSignature {
        heading_millirad: if active_shape_rotates_with_heading(rs) {
            quantize(rs.core.heading, 1000.0)
        } else {
            0
        },
        idle_alpha_milli: quantize(rs.core.idle_alpha, 1000.0),
        pressed: rs.core.pressed,
        click_milli: rs.core.click_t.map(|t| quantize(t, 1000.0)).unwrap_or(-1),
        focus_milli: if rs.focus_rect.is_some() {
            quantize(rs.focus_rect_t, 1000.0)
        } else {
            -1
        },
        builtin_shape: builtin_shape_code(&rs.core),
        custom_shape: rs
            .core
            .shape
            .as_ref()
            .map(|shape| (shape.width, shape.height)),
    }
}

fn cursor_window_rect(rs: &RenderState) -> Option<LogicalRect> {
    if !rs.core.visible || rs.core.pos.0 < -100.0 || rs.core.idle_alpha < 0.004 {
        return None;
    }

    let mut rect = LogicalRect::around(rs.core.pos, CURSOR_WINDOW_MARGIN_POINTS);
    if let Some([x, y, w, h]) = rs.focus_rect {
        let focus = LogicalRect {
            left: x,
            top: y,
            width: w.max(1.0),
            height: h.max(1.0),
        }
        .expanded(FOCUS_RECT_MARGIN_POINTS);
        rect = rect.union(focus);
    }
    Some(rect)
}

fn cursor_redraw_needed(
    last_appearance: Option<AppearanceSignature>,
    last_pixel_size: Option<(u32, u32)>,
    last_backing_scale: Option<f64>,
    next_appearance: AppearanceSignature,
    next_pixel_size: (u32, u32),
    next_backing_scale: f64,
    force: bool,
    focus_active: bool,
) -> bool {
    force
        || focus_active
        || last_appearance != Some(next_appearance)
        || last_pixel_size != Some(next_pixel_size)
        || backing_scale_changed(last_backing_scale, next_backing_scale)
}

fn render_cursor_pixmap(
    rs: &RenderState,
    rect: LogicalRect,
    backing_scale: f64,
) -> tiny_skia::Pixmap {
    let scale = backing_scale.max(1.0);
    let (w, h) = rect.pixel_size(scale);
    let mut pm =
        tiny_skia::Pixmap::new(w, h).unwrap_or_else(|| tiny_skia::Pixmap::new(1, 1).unwrap());
    let focus = rs.focus_rect.map(|focus_rect| FocusRect {
        rect: focus_rect,
        t: rs.focus_rect_t,
    });
    cursor_overlay::paint_cursor(&mut pm, &rs.core, rect.left, rect.top, focus, scale as f32);
    pm
}

// ── AppKit / CGImage plumbing ─────────────────────────────────────────────

unsafe fn run_appkit(_cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayMsg>) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSRect;

    // ---- NSApplication ----
    // Verify main thread (MainThreadMarker is a zero-size compile-time token).
    let _mtm = objc2_foundation::MainThreadMarker::new()
        .expect("run_appkit must be called from the main thread");

    let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
    // NSApplicationActivationPolicyAccessory = 1 (no Dock icon, no menu bar)
    // setActivationPolicy: returns BOOL (success), not void.
    let _: bool = msg_send![app, setActivationPolicy: 1i64];
    // Finish launching without presenting a UI before pumping AppKit events.
    let _: () = msg_send![app, finishLaunching];

    // ---- Main screen frame ----
    let main_screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
    if main_screen.is_null() {
        // Headless environment (CI without display) — skip overlay entirely.
        // The MCP server continues on the background thread.
        return;
    }
    let screen_frame: NSRect = msg_send![main_screen, frame];
    let win_w = screen_frame.size.width;
    let win_h = screen_frame.size.height;
    // NSScreen.backingScaleFactor is the most direct source of truth — it's
    // what AppKit will use for the layer's native backing surface anyway.
    // Fall back to the CG estimator (pixel mode width ÷ logical bounds) when
    // the AppKit call returns a non-positive value, since downstream paint
    // math divides by this and a 0.0 would zero out the cursor.
    let mut backing_scale: f64 = msg_send![main_screen, backingScaleFactor];
    if !(backing_scale > 0.0) {
        use core_graphics::display::{CGDisplayBounds, CGMainDisplayID};
        let display_id = CGMainDisplayID();
        let bounds = CGDisplayBounds(display_id);
        backing_scale =
            crate::tools::get_screen_size::get_backing_scale(display_id, bounds.size.width as i64);
        if !(backing_scale > 0.0) {
            backing_scale = 1.0;
        }
    }
    let max_fps: i64 = msg_send![main_screen, maximumFramesPerSecond];
    let frame_budget = frame_budget_for_max_fps(max_fps);

    // Keep the main screen as the global top-left coordinate anchor. Raster
    // density is selected independently per cursor from every active display.
    {
        let mut guard = RENDER.lock().unwrap();
        if let Some(m) = guard.as_mut() {
            m.win_w = win_w;
            m.win_h = win_h;
        }
    }

    let mut displays = active_display_geometries(backing_scale);
    if displays.is_empty() {
        displays.push(DisplayGeometry {
            bounds: LogicalRect {
                left: 0.0,
                top: 0.0,
                width: win_w,
                height: win_h,
            },
            backing_scale,
        });
    }

    // ---- Render thread (display-rate while animating, quiescent while idle) ----
    let screen = ScreenGeometry {
        origin_x: screen_frame.origin.x,
        origin_y: screen_frame.origin.y,
        height: win_h,
        fallback_backing_scale: backing_scale,
    };
    std::thread::spawn(move || {
        render_loop(rx, screen, displays, frame_budget);
    });

    // ---- NSApplication run loop ----
    if !crate::pip::appkit_main_loop_stop_requested() {
        crate::pip::mark_appkit_main_loop_running();
        crate::pip::run_appkit_event_loop(app);
    }
    crate::pip::mark_appkit_main_loop_stopped();
}

struct CursorWindowHandle {
    win_ptr: usize,
    layer_ptr: usize,
    visible: bool,
    last_appearance: Option<AppearanceSignature>,
    last_pixel_size: Option<(u32, u32)>,
    last_backing_scale: f64,
    last_pinned: Option<u64>,
    last_level: Option<PanelLevel>,
    repin_frames: u32,
}

struct CursorWindowUpdate {
    key: CursorKey,
    rect: Option<LogicalRect>,
    appearance: Option<AppearanceSignature>,
    pixel_size: Option<(u32, u32)>,
    backing_scale: f64,
    pixmap: Option<tiny_skia::Pixmap>,
    pinned_wid: Option<u64>,
    panel_level: PanelLevel,
}

fn render_loop(
    rx: std::sync::mpsc::Receiver<OverlayMsg>,
    screen: ScreenGeometry,
    mut displays: Vec<DisplayGeometry>,
    target_frame_ms: Duration,
) {
    let mut last_tick = Instant::now();
    let mut frame_tick_needed = false;
    let mut idle_fade_due_in: Option<Duration> = None;
    let mut handles: HashMap<CursorKey, CursorWindowHandle> = HashMap::new();
    let mut pinned_bounds = HashMap::new();
    let mut last_geometry_refresh: Option<Instant> = None;
    let repin_interval_frames = repin_frame_interval(target_frame_ms);

    loop {
        // When no cursor animation/fade is active, either block until the MCP
        // side sends a command or, for an idle-hide dwell, sleep only until the
        // fade is due. This avoids 60fps fullscreen compositing while the
        // cursor is fully visible but static.
        let mut woke_for_idle_fade = false;
        let first_msg = match (frame_tick_needed, idle_fade_due_in) {
            (true, _) => None,
            (false, Some(timeout)) => match rx.recv_timeout(timeout) {
                Ok(msg) => Some(msg),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    woke_for_idle_fade = true;
                    None
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            },
            (false, None) => match rx.recv() {
                Ok(msg) => Some(msg),
                Err(_) => break,
            },
        };

        let woke_from_idle = first_msg.is_some();
        let now = Instant::now();
        if woke_from_idle
            || last_geometry_refresh
                .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(1))
        {
            let refreshed_displays = active_display_geometries(screen.fallback_backing_scale);
            if !refreshed_displays.is_empty() {
                displays = refreshed_displays;
            }
            pinned_bounds = window_bounds_snapshot();
            last_geometry_refresh = Some(now);
        }
        let dt = if woke_from_idle {
            // The blocking recv() above can span an arbitrarily long idle period.
            // Do not charge that time to the first animation tick after a command;
            // let the wake-up frame render the newly-applied state at t=0.
            0.0
        } else if woke_for_idle_fade {
            // The dwell timeout exists solely to advance idle_secs to fade_start.
            // Paths/springs/clicks are absent while dwelling, so feeding the full
            // slept wall-clock duration into tick_idle is safe and intentional.
            now.duration_since(last_tick).as_secs_f64()
        } else {
            now.duration_since(last_tick).as_secs_f64().min(0.05)
        };
        last_tick = now;

        // ── Phase 1: drain + tick all cursors, then prepare small-window updates.
        // `arrived` collects keys whose path just ended. Position-only motion
        // updates only the per-cursor panel frame; bitmap redraws are reserved
        // for appearance changes such as heading, click pulse, focus fade, or
        // idle fade alpha.
        let (arrived, had_msg, next_frame_tick_needed, next_idle_fade_due_in, updates) = {
            let mut guard = RENDER.lock().unwrap();
            match guard.as_mut() {
                Some(map) => {
                    let mut had_msg = false;
                    if let Some(msg) = first_msg {
                        had_msg = true;
                        note_command_dequeued(&msg);
                        let _ = apply_msg(map, msg);
                    }
                    while let Ok(msg) = rx.try_recv() {
                        had_msg = true;
                        note_command_dequeued(&msg);
                        let _ = apply_msg(map, msg);
                    }
                    // Tick every cursor while an animation/fade is in progress
                    // or immediately after a command changed render state. The
                    // latter lets a just-created path/click/focus rect start on
                    // this frame without waiting for the next 16ms tick.
                    let mut arrived: Vec<CursorKey> = Vec::new();
                    if frame_tick_needed || had_msg || woke_for_idle_fade {
                        for (k, rs) in map.cursors.iter_mut() {
                            if rs.tick(dt) {
                                arrived.push(k.clone());
                            }
                        }
                    }
                    let next_frame_tick_needed = render_map_needs_frame_tick(map);
                    let next_idle_fade_due_in = if next_frame_tick_needed {
                        None
                    } else {
                        render_map_idle_fade_due_in(map)
                    };

                    let updates = map
                        .cursors
                        .iter()
                        .map(|(key, rs)| {
                            let pinned_target = rs
                                .core
                                .pinned_wid
                                .and_then(|window_id| pinned_bounds.get(&window_id).copied());
                            let backing_scale = backing_scale_for_cursor(
                                &displays,
                                screen.fallback_backing_scale,
                                rs.core.pos,
                                pinned_target,
                            );
                            let rect = cursor_window_rect(rs);
                            let (appearance, pixel_size, pixmap) = if let Some(rect) = rect {
                                let appearance = appearance_signature(rs);
                                let pixel_size = rect.pixel_size(backing_scale);
                                let handle = handles.get(key);
                                let redraw = cursor_redraw_needed(
                                    handle.and_then(|h| h.last_appearance),
                                    handle.and_then(|h| h.last_pixel_size),
                                    handle.map(|h| h.last_backing_scale),
                                    appearance,
                                    pixel_size,
                                    backing_scale,
                                    had_msg,
                                    rs.focus_rect.is_some(),
                                );
                                let pixmap =
                                    redraw.then(|| render_cursor_pixmap(rs, rect, backing_scale));
                                (Some(appearance), Some(pixel_size), pixmap)
                            } else {
                                (None, None, None)
                            };
                            CursorWindowUpdate {
                                key: key.clone(),
                                rect,
                                appearance,
                                pixel_size,
                                backing_scale,
                                pixmap,
                                pinned_wid: rs.core.pinned_wid,
                                panel_level: panel_level_for_pin(rs.core.pinned_wid),
                            }
                        })
                        .collect::<Vec<_>>();

                    (
                        arrived,
                        had_msg,
                        next_frame_tick_needed,
                        next_idle_fade_due_in,
                        updates,
                    )
                }
                None => break,
            }
        };

        // Fire arrival signals so each session's animate_cursor_to() unblocks.
        for k in &arrived {
            arrival_fire(k);
        }

        // ── Phase 2: move per-cursor windows and update contents only when needed.
        for update in updates {
            match update.rect {
                Some(rect) => {
                    let handle = match handles.get_mut(&update.key) {
                        Some(handle) => handle,
                        None => {
                            let Some(new_handle) =
                                dispatch_create_cursor_window(update.backing_scale)
                            else {
                                continue;
                            };
                            handles.insert(update.key.clone(), new_handle);
                            handles.get_mut(&update.key).unwrap()
                        }
                    };

                    let frame = appkit_frame_for_rect(rect, screen);
                    let was_visible = handle.visible;
                    let pin_changed = update.pinned_wid != handle.last_pinned;
                    let level_changed = Some(update.panel_level) != handle.last_level;
                    let contents_scale = backing_scale_changed(
                        Some(handle.last_backing_scale),
                        update.backing_scale,
                    )
                    .then_some(update.backing_scale);
                    dispatch_update_cursor_window(
                        handle.win_ptr,
                        handle.layer_ptr,
                        frame,
                        contents_scale,
                        update.pixmap,
                        update.panel_level,
                        update.pinned_wid,
                        !was_visible || pin_changed || level_changed,
                    );
                    handle.visible = true;
                    handle.last_appearance = update.appearance;
                    handle.last_pixel_size = update.pixel_size;
                    handle.last_backing_scale = update.backing_scale;
                    handle.last_level = Some(update.panel_level);

                    if frame_tick_needed || had_msg || !was_visible {
                        handle.repin_frames += 1;
                    }
                    handle.last_pinned = update.pinned_wid;
                    if update.pinned_wid.is_some() && handle.repin_frames >= repin_interval_frames {
                        MacZOrderEnforcer {
                            win_ptr: handle.win_ptr,
                        }
                        .reassert(update.pinned_wid);
                        handle.repin_frames = 0;
                    } else if handle.repin_frames >= repin_interval_frames {
                        handle.repin_frames = 0;
                    }
                }
                None => {
                    if let Some(handle) = handles.get_mut(&update.key) {
                        if handle.visible {
                            dispatch_hide_cursor_window(handle.win_ptr);
                            handle.visible = false;
                        }
                        handle.last_appearance = None;
                        handle.last_pixel_size = None;
                    }
                }
            }
        }

        let live_keys = {
            let guard = RENDER.lock().unwrap();
            guard
                .as_ref()
                .map(|map| {
                    map.cursors
                        .keys()
                        .cloned()
                        .collect::<std::collections::HashSet<_>>()
                })
                .unwrap_or_default()
        };
        handles.retain(|key, handle| {
            if live_keys.contains(key) {
                true
            } else {
                dispatch_close_cursor_window(handle.win_ptr);
                false
            }
        });

        frame_tick_needed = next_frame_tick_needed;
        idle_fade_due_in = next_idle_fade_due_in;
        if frame_tick_needed {
            // Sleep remainder of frame budget.
            let elapsed = Instant::now().duration_since(last_tick);
            if let Some(remaining) = target_frame_ms.checked_sub(elapsed) {
                std::thread::sleep(remaining);
            }
        }
    }
}

fn dispatch_create_cursor_window(backing_scale: f64) -> Option<CursorWindowHandle> {
    #[link(name = "dispatch", kind = "dylib")]
    extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_sync_f(
            queue: *const c_void,
            context: *mut c_void,
            work: unsafe extern "C" fn(*mut c_void),
        );
    }

    struct Payload {
        backing_scale: f64,
        win_ptr: usize,
        layer_ptr: usize,
    }

    unsafe extern "C" fn create_cb(ctx: *mut c_void) {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        let payload = &mut *(ctx as *mut Payload);
        let rect = NSRect {
            origin: NSPoint {
                x: -10_000.0,
                y: -10_000.0,
            },
            size: NSSize {
                width: 1.0,
                height: 1.0,
            },
        };

        let allocated: *mut AnyObject = msg_send![class!(NSPanel), alloc];
        // NSWindowStyleMaskNonactivatingPanel = 1 << 7.
        let win: *mut AnyObject = msg_send![allocated,
            initWithContentRect: rect
            styleMask: (1u64 << 7)
            backing: 2u64
            defer: false
        ];
        if win.is_null() {
            return;
        }

        let _: () = msg_send![win, setOpaque: false];
        let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
        let _: () = msg_send![win, setBackgroundColor: clear];
        let _: () = msg_send![win, setHasShadow: false];
        let _: () = msg_send![win, setIgnoresMouseEvents: true];
        // NSFloatingWindowLevel = 3: visible above normal app windows without
        // activating this accessory app.
        let _: () = msg_send![win, setLevel: 3i64];
        // CanJoinAllSpaces | FullScreenAuxiliary | Stationary.
        let _: () = msg_send![win, setCollectionBehavior: (1u64 | (1 << 8) | (1 << 4))];
        let _: () = msg_send![win, setReleasedWhenClosed: false];
        let _: () = msg_send![win, setHidesOnDeactivate: false];

        let content_view: *mut AnyObject = msg_send![win, contentView];
        let _: () = msg_send![content_view, setWantsLayer: true];
        let layer: *mut AnyObject = msg_send![content_view, layer];
        let _: () = msg_send![layer, setContentsScale: payload.backing_scale.max(1.0)];
        let gravity_ns: *mut AnyObject = msg_send![class!(NSString),
            stringWithUTF8String: b"topLeft\0".as_ptr()
        ];
        let _: () = msg_send![layer, setContentsGravity: gravity_ns];
        let _: () = msg_send![win, orderOut: std::ptr::null_mut::<AnyObject>()];

        payload.win_ptr = win as usize;
        payload.layer_ptr = layer as usize;
    }

    let mut payload = Payload {
        backing_scale,
        win_ptr: 0,
        layer_ptr: 0,
    };
    unsafe {
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_sync_f(
            main_queue,
            &mut payload as *mut Payload as *mut c_void,
            create_cb,
        );
    }
    if payload.win_ptr == 0 || payload.layer_ptr == 0 {
        None
    } else {
        Some(CursorWindowHandle {
            win_ptr: payload.win_ptr,
            layer_ptr: payload.layer_ptr,
            visible: false,
            last_appearance: None,
            last_pixel_size: None,
            last_backing_scale: normalized_backing_scale(backing_scale, 1.0),
            last_pinned: None,
            last_level: None,
            repin_frames: 0,
        })
    }
}

fn dispatch_update_cursor_window(
    win_ptr: usize,
    layer_ptr: usize,
    frame: AppKitRect,
    contents_scale: Option<f64>,
    pixmap: Option<tiny_skia::Pixmap>,
    panel_level: PanelLevel,
    order_target: Option<u64>,
    should_order: bool,
) {
    let cg_image_ptr = pixmap.as_ref().and_then(pixmap_to_cgimage).unwrap_or(0);
    drop(pixmap);

    #[link(name = "dispatch", kind = "dylib")]
    extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_async_f(
            queue: *const c_void,
            context: *mut c_void,
            work: unsafe extern "C" fn(*mut c_void),
        );
    }

    struct Payload {
        win_ptr: usize,
        layer_ptr: usize,
        frame: AppKitRect,
        contents_scale: Option<f64>,
        cg_image_ptr: usize,
        panel_level: PanelLevel,
        order_target: Option<u64>,
        should_order: bool,
    }

    unsafe extern "C" fn update_cb(ctx: *mut c_void) {
        use objc2::runtime::AnyObject;
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        let payload: Payload = *Box::from_raw(ctx as *mut Payload);
        let win = payload.win_ptr as *mut AnyObject;
        let layer = payload.layer_ptr as *mut AnyObject;
        let frame = NSRect {
            origin: NSPoint {
                x: payload.frame.x,
                y: payload.frame.y,
            },
            size: NSSize {
                width: payload.frame.width,
                height: payload.frame.height,
            },
        };
        let _: () = objc2::msg_send![win, setFrame: frame display: false];
        let _: () = objc2::msg_send![win, setLevel: payload.panel_level.ns_window_level()];
        if let Some(contents_scale) = payload.contents_scale {
            let _: () = objc2::msg_send![layer, setContentsScale: contents_scale];
        }
        if payload.cg_image_ptr != 0 {
            let cg_id = payload.cg_image_ptr as *mut AnyObject;
            let _: () = objc2::msg_send![layer, setContents: cg_id];
            CGImageRelease(payload.cg_image_ptr as *mut c_void);
        }
        if payload.should_order {
            let relative_to = payload.order_target.unwrap_or(0) as i64;
            let _: () = objc2::msg_send![win, orderWindow: 1i64 relativeTo: relative_to];
        }
    }

    extern "C" {
        fn CGImageRelease(image: *mut c_void);
    }

    let payload = Box::new(Payload {
        win_ptr,
        layer_ptr,
        frame,
        contents_scale: contents_scale.map(|scale| normalized_backing_scale(scale, 1.0)),
        cg_image_ptr,
        panel_level,
        order_target,
        should_order,
    });
    unsafe {
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(main_queue, Box::into_raw(payload) as *mut c_void, update_cb);
    }
}

fn dispatch_hide_cursor_window(win_ptr: usize) {
    dispatch_window_lifecycle(win_ptr, false);
}

fn dispatch_close_cursor_window(win_ptr: usize) {
    dispatch_window_lifecycle(win_ptr, true);
}

fn dispatch_window_lifecycle(win_ptr: usize, close: bool) {
    #[link(name = "dispatch", kind = "dylib")]
    extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_async_f(
            queue: *const c_void,
            context: *mut c_void,
            work: unsafe extern "C" fn(*mut c_void),
        );
    }

    unsafe extern "C" fn lifecycle_cb(ctx: *mut c_void) {
        let (win_ptr, close): (usize, bool) = *Box::from_raw(ctx as *mut (usize, bool));
        let win = win_ptr as *mut objc2::runtime::AnyObject;
        let _: () =
            objc2::msg_send![win, orderOut: std::ptr::null_mut::<objc2::runtime::AnyObject>()];
        if close {
            let _: () = objc2::msg_send![win, close];
            // `dispatch_create_cursor_window` owns the +1 from alloc/init and
            // sets releasedWhenClosed=false. Balance that ownership here, on
            // the AppKit main queue, after all earlier updates for this window.
            let _: () = objc2::msg_send![win, release];
        }
    }

    let payload = Box::new((win_ptr, close));
    unsafe {
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(
            main_queue,
            Box::into_raw(payload) as *mut c_void,
            lifecycle_cb,
        );
    }
}

/// Order the overlay NSWindow just above `target_wid` in the global window
/// server list.  Called from the render thread; dispatches to the main queue
/// (AppKit must be used on the main thread).
///
/// `NSWindowAbove = 1`; `orderWindow:relativeTo:` accepts any CGWindowID as
/// the `relativeTo` argument — it works cross-application via CGS.
fn dispatch_pin_above(win_ptr: usize, target_wid: u64) {
    use std::ffi::c_void;

    #[link(name = "dispatch", kind = "dylib")]
    extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_async_f(
            queue: *const c_void,
            context: *mut c_void,
            work: unsafe extern "C" fn(*mut c_void),
        );
    }

    unsafe extern "C" fn reorder_cb(ctx: *mut c_void) {
        let (win_ptr, target_wid): (usize, u64) = *Box::from_raw(ctx as *mut (usize, u64));
        let win = win_ptr as *mut objc2::runtime::AnyObject;
        // NSWindowAbove = 1; relativeTo: takes NSInteger (i64 on 64-bit)
        let _: () = objc2::msg_send![win, orderWindow: 1i64 relativeTo: target_wid as i64];
    }

    let payload = Box::new((win_ptr, target_wid));
    unsafe {
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(
            main_queue,
            Box::into_raw(payload) as *mut c_void,
            reorder_cb,
        );
    }
}

// ── Z-order enforcer (macOS impl of cursor_overlay::ZOrderEnforcer) ──────

/// macOS implementation of [`cursor_overlay::ZOrderEnforcer`].
///
/// Holds the NSWindow pointer as a `usize` and dispatches the
/// `orderWindow:relativeTo:` call to the main queue (AppKit must run on
/// the main thread).
///
/// `target = None` is treated as a no-op here: free-move visibility is handled
/// by `PanelLevel::Floating`, while pinned panels are placed at the normal
/// window level and ordered relative to the target CGWindow id.
struct MacZOrderEnforcer {
    win_ptr: usize,
}

impl ZOrderEnforcer for MacZOrderEnforcer {
    fn reassert(&self, target: Option<u64>) {
        if let Some(wid) = target {
            dispatch_pin_above(self.win_ptr, wid);
        }
        // target = None → no-op; see struct doc comment.
    }
}

/// Create a `CGImage` from a `tiny_skia::Pixmap` (premultiplied RGBA).
/// Returns a `+1` retained pointer that the caller must release.
fn pixmap_to_cgimage(pixmap: &tiny_skia::Pixmap) -> Option<usize> {
    let w = pixmap.width() as usize;
    let h = pixmap.height() as usize;
    if w == 0 || h == 0 {
        return None;
    }

    let data = pixmap.data();
    let bytes_per_row = w * 4;

    // tiny-skia produces premultiplied RGBA with bytes in memory order [R, G, B, A].
    // CGImage flag breakdown (Apple CGBitmapInfo / CGImageAlphaInfo enums):
    //   kCGImageAlphaPremultipliedLast = 0x0001  → alpha is the LAST channel  (RGBA)
    //   kCGImageAlphaPremultipliedFirst = 0x0002 → alpha is the FIRST channel (ARGB)  ← NOT what we want
    //   kCGBitmapByteOrder32Big        = 0x4000  → big-endian 32-bit pixel,
    //     so memory order is the same as component order (bytes = [R, G, B, A]).
    // Combined: kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big = 0x4001
    // This correctly maps tiny-skia's [R, G, B, A] bytes to the display RGB channels.
    const BITMAP_INFO: u32 = 0x0001 | 0x4000; // kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big

    // Release callback: CGDataProvider calls this when it is done with the buffer.
    // `info` is the Box<Vec<u8>> we passed as the `info` argument below.
    unsafe extern "C" fn release_pixel_data(info: *mut c_void, _data: *const c_void, _size: usize) {
        // Re-box and drop to free the buffer.
        drop(Box::from_raw(info as *mut Vec<u8>));
    }

    unsafe {
        extern "C" {
            fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
            fn CGColorSpaceRelease(cs: *mut c_void);
            fn CGDataProviderCreateWithData(
                info: *mut c_void,
                data: *const c_void,
                size: usize,
                release_data: Option<unsafe extern "C" fn(*mut c_void, *const c_void, usize)>,
            ) -> *mut c_void;
            fn CGDataProviderRelease(provider: *mut c_void);
            fn CGImageCreate(
                width: usize,
                height: usize,
                bits_per_component: usize,
                bits_per_pixel: usize,
                bytes_per_row: usize,
                color_space: *mut c_void,
                bitmap_info: u32,
                provider: *mut c_void,
                decode: *const f64,
                should_interpolate: bool,
                intent: u32,
            ) -> *mut c_void;
        }

        // Copy the pixel data into a heap Vec; the data provider will own it
        // and free it via release_pixel_data when the CGImage is released.
        let copied: Vec<u8> = data.to_vec();
        let len = copied.len();
        let ptr = copied.as_ptr();
        // Leak the Vec into a raw Box so we can pass it as the `info` opaque pointer.
        let copied_box: *mut Vec<u8> = Box::into_raw(Box::new(copied));

        let cs = CGColorSpaceCreateDeviceRGB();
        let provider = CGDataProviderCreateWithData(
            copied_box as *mut c_void,
            ptr as *const c_void,
            len,
            Some(release_pixel_data), // frees copied_box when provider is released
        );
        let img = CGImageCreate(
            w,
            h,
            8,  // bits_per_component
            32, // bits_per_pixel
            bytes_per_row,
            cs,
            BITMAP_INFO,
            provider,
            std::ptr::null(),
            false,
            0, // kCGRenderingIntentDefault
        );

        CGColorSpaceRelease(cs);
        CGDataProviderRelease(provider);
        // Do NOT drop copied_box here — release_pixel_data owns it now.

        if img.is_null() {
            None
        } else {
            Some(img as usize)
        }
    }
}

// ── Headless unit tests for the keyed render collection ───────────────────
//
// These prove the per-session ownership data model, the session_end removal
// lifecycle, the "default" guard, and per-key arrival isolation WITHOUT any
// AppKit / NSWindow. The on-screen rendering (CGImage / CALayer setContents)
// still needs a real display and is verified separately on the macOS VM.

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn empty_map() -> RenderMap {
        let mut cursors = IndexMap::new();
        cursors.insert(
            "default".to_owned(),
            RenderState::new(CursorConfig::default()),
        );
        RenderMap {
            cursors,
            last_active_key: None,
            win_w: 100.0,
            win_h: 100.0,
            template: CursorConfig::default(),
            ended: std::collections::HashSet::new(),
        }
    }

    fn move_msg(key: &str, x: f64, y: f64) -> OverlayMsg {
        OverlayMsg::Cmd(KeyedOverlayCommand {
            key: key.to_owned(),
            cmd: OverlayCommand::MoveTo {
                x,
                y,
                end_heading_radians: 0.0,
            },
        })
    }

    #[test]
    fn two_sessions_produce_two_distinct_render_entries() {
        let mut map = empty_map();
        apply_msg(
            &mut map,
            OverlayMsg::Cmd(KeyedOverlayCommand {
                key: "sessA".to_owned(),
                cmd: OverlayCommand::SetEnabled(true),
            }),
        );
        apply_msg(&mut map, move_msg("sessB", 42.0, 24.0));
        // default + sessA + sessB = 3 distinct owned cursors (the core
        // regression today is that they would clobber to one).
        assert_eq!(map.cursors.len(), 3);
        assert!(map.cursors.contains_key("sessA"));
        assert!(map.cursors.contains_key("sessB"));
        assert!(map.cursors.contains_key("default"));
    }

    #[test]
    fn session_end_removes_only_that_session() {
        let mut map = empty_map();
        apply_msg(&mut map, move_msg("sessA", 10.0, 10.0));
        apply_msg(&mut map, move_msg("sessB", 20.0, 20.0));
        assert_eq!(map.cursors.len(), 3);

        // session_end(A): A gone, B + default retained.
        apply_msg(&mut map, OverlayMsg::Remove("sessA".to_owned()));
        assert!(!map.cursors.contains_key("sessA"));
        assert!(map.cursors.contains_key("sessB"));
        assert!(map.cursors.contains_key("default"));
        assert_eq!(map.cursors.len(), 2);

        // Remove("default") is guarded — default survives.
        apply_msg(&mut map, OverlayMsg::Remove("default".to_owned()));
        assert!(map.cursors.contains_key("default"));

        // Remove of an absent key (anonymous session that never created a
        // cursor) is a harmless no-op.
        let before = map.cursors.len();
        apply_msg(&mut map, OverlayMsg::Remove("never-existed".to_owned()));
        assert_eq!(map.cursors.len(), before);
    }

    #[test]
    fn lazily_created_cursors_get_distinct_palettes() {
        let mut map = empty_map();
        apply_msg(&mut map, move_msg("sessA", 10.0, 10.0));
        apply_msg(&mut map, move_msg("sessB", 20.0, 20.0));
        let a = &map.cursors["sessA"].core.palette;
        let b = &map.cursors["sessB"].core.palette;
        let def = &map.cursors["default"].core.palette;
        // default uses the blue palette; the two sessions derive their own.
        assert_ne!(a.name, def.name);
        assert_ne!(b.name, def.name);
    }

    #[test]
    fn insertion_order_is_stable_z_order() {
        let mut map = empty_map();
        apply_msg(&mut map, move_msg("first", 1.0, 1.0));
        apply_msg(&mut map, move_msg("second", 2.0, 2.0));
        // Re-touching "first" must NOT move it to the back (IndexMap keeps the
        // original insertion slot), so z-order is stable frame to frame.
        apply_msg(&mut map, move_msg("first", 3.0, 3.0));
        let keys: Vec<&String> = map.cursors.keys().collect();
        assert_eq!(keys, vec!["default", "first", "second"]);
    }

    #[test]
    fn tombstone_blocks_late_command_then_explicit_revive_accepts_new_command() {
        // The resurrection race: a Cmd for a session lands AFTER its Remove
        // (an in-flight click from another task as the session dies). The
        // tombstone must drop it so the just-removed cursor is NOT re-created.
        let mut map = empty_map();
        apply_msg(&mut map, move_msg("sessA", 10.0, 10.0));
        assert_eq!(map.cursors.len(), 2); // default + sessA

        // Session ends → cursor removed, key tombstoned.
        apply_msg(&mut map, OverlayMsg::Remove("sessA".to_owned()));
        assert!(!map.cursors.contains_key("sessA"));
        assert_eq!(map.cursors.len(), 1);

        // A late in-flight Cmd for the ended session must be dropped WITHOUT
        // re-inserting (no get-or-create resurrection).
        let resolved = apply_msg(&mut map, move_msg("sessA", 99.0, 99.0));
        assert!(
            resolved.is_none(),
            "ended-session Cmd must be dropped, not resolved"
        );
        assert!(
            !map.cursors.contains_key("sessA"),
            "tombstone must block resurrection"
        );
        assert_eq!(
            map.cursors.len(),
            1,
            "render map length must stay at default only"
        );

        // An explicit start_session revival is ordered after the late command.
        // It clears only this key's tombstone, so the next command can lazily
        // create a fresh cursor again.
        apply_msg(&mut map, OverlayMsg::Revive("sessA".to_owned()));
        assert!(!map.ended.contains("sessA"));
        let resolved = apply_msg(&mut map, move_msg("sessA", 55.0, 66.0));
        assert_eq!(resolved.as_deref(), Some("sessA"));
        assert!(map.cursors.contains_key("sessA"));
    }

    #[tokio::test]
    async fn failed_move_send_and_timeout_both_clear_the_registered_waiter() {
        static ARRIVAL_TEST_LOCK: Mutex<()> = Mutex::new(());
        let _test_guard = ARRIVAL_TEST_LOCK.lock().unwrap();
        *ARRIVAL_TX.lock().unwrap() = Some(HashMap::new());

        // A stopped renderer drops its receiver, so command delivery fails
        // immediately without leaving the registered arrival behind.
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        drop(command_rx);

        let key = "send-failure".to_owned();
        let (arrival_tx, arrival_rx) = tokio::sync::oneshot::channel();
        let generation = arrival_register(key.clone(), arrival_tx);
        let sent = send_registered_move(
            Some(&command_tx),
            key.clone(),
            OverlayCommand::MoveTo {
                x: 2.0,
                y: 2.0,
                end_heading_radians: 0.0,
            },
            generation,
        );
        assert!(!sent);
        assert!(arrival_rx.await.is_err(), "cancel must close the waiter");
        assert!(!ARRIVAL_TX
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .contains_key(&key));

        // A renderer that accepts the command but never reports arrival is
        // bounded by the cancellable timeout and leaves no stale waiter.
        let key = "arrival-timeout".to_owned();
        let (arrival_tx, arrival_rx) = tokio::sync::oneshot::channel();
        let generation = arrival_register(key.clone(), arrival_tx);
        assert!(
            !wait_for_arrival(&key, generation, arrival_rx, Duration::ZERO).await,
            "pending arrival must time out"
        );
        assert!(!ARRIVAL_TX
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .contains_key(&key));

        *ARRIVAL_TX.lock().unwrap() = None;
    }

    #[test]
    fn lifecycle_events_remain_ordered_behind_a_saturated_renderer_backlog() {
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let pending = std::sync::atomic::AtomicUsize::new(0);

        // Saturate the command budget while the renderer is not draining.
        // Further render updates are dropped, but lifecycle events still enter
        // the same ordered queue without blocking.
        for index in 0..MAX_PENDING_COMMANDS {
            assert!(try_send_command_with_depth(
                Some(&command_tx),
                "sessA".to_owned(),
                OverlayCommand::MoveTo {
                    x: index as f64,
                    y: 0.0,
                    end_heading_radians: 0.0,
                },
                &pending,
            ));
        }
        assert!(!try_send_command_with_depth(
            Some(&command_tx),
            "sessA".to_owned(),
            OverlayCommand::MoveTo {
                x: -1.0,
                y: 0.0,
                end_heading_radians: 0.0,
            },
            &pending,
        ));
        command_tx
            .send(OverlayMsg::Remove("sessA".to_owned()))
            .unwrap();
        command_tx
            .send(OverlayMsg::Revive("sessA".to_owned()))
            .unwrap();

        for _ in 0..MAX_PENDING_COMMANDS {
            assert!(matches!(command_rx.recv().unwrap(), OverlayMsg::Cmd(_)));
        }
        assert!(matches!(
            command_rx.recv().unwrap(),
            OverlayMsg::Remove(key) if key == "sessA"
        ));
        assert!(matches!(
            command_rx.recv().unwrap(),
            OverlayMsg::Revive(key) if key == "sessA"
        ));
    }

    #[test]
    fn default_is_never_tombstoned() {
        // Remove("default") is guarded, so default is never tombstoned and a
        // subsequent Cmd on default still renders.
        let mut map = empty_map();
        apply_msg(&mut map, OverlayMsg::Remove("default".to_owned()));
        assert!(map.cursors.contains_key("default"));
        assert!(!map.ended.contains("default"));

        let resolved = apply_msg(&mut map, move_msg("default", 5.0, 5.0));
        assert_eq!(resolved.as_deref(), Some("default"));
        assert!(map.cursors.contains_key("default"));
    }

    #[test]
    fn seed_moves_sentinel_cursor_on_screen_for_first_action() {
        // BUG 2 regression: a brand-new session cursor at the sentinel must be
        // seeded on-screen (pos.0 > -50) so the immediately-following MoveTo
        // glides instead of silently snapping via ClickPulse.
        let mut map = empty_map(); // 100x100 frame
                                   // No "sessA" cursor exists yet — the seed must get-or-create it.
        let seeded = seed_start_in_map(&mut map, &"sessA".to_owned(), 60.0, 60.0);
        assert!(seeded, "sentinel cursor must be seeded");
        let pos = map.cursors["sessA"].core.pos;
        assert!(
            pos.0 > -50.0 && pos.1 > -50.0,
            "seed must be on-screen, got {pos:?}"
        );
        // And it must be a DIFFERENT point from the target so there is a glide.
        assert!(
            (pos.0 - 60.0).abs() > 4.0 || (pos.1 - 60.0).abs() > 4.0,
            "seed must differ from target to produce a visible glide, got {pos:?}"
        );
    }

    #[test]
    fn seed_is_noop_when_cursor_already_on_screen() {
        // A second action: the cursor already landed somewhere on-screen, so the
        // seed must NOT move it (the MoveTo path should start from where it is).
        let mut map = empty_map();
        // Put sessA on-screen first.
        seed_start_in_map(&mut map, &"sessA".to_owned(), 60.0, 60.0);
        map.cursors.get_mut("sessA").unwrap().core.pos = (30.0, 30.0);
        let seeded_again = seed_start_in_map(&mut map, &"sessA".to_owned(), 80.0, 80.0);
        assert!(!seeded_again, "on-screen cursor must not be re-seeded");
        assert_eq!(
            map.cursors["sessA"].core.pos,
            (30.0, 30.0),
            "pos must be untouched"
        );
    }

    #[test]
    fn seed_does_not_resurrect_ended_session() {
        // The seed shares the resurrection guard: it must not re-create a cursor
        // whose session already ended.
        let mut map = empty_map();
        map.ended.insert("sessA".to_owned());
        let seeded = seed_start_in_map(&mut map, &"sessA".to_owned(), 60.0, 60.0);
        assert!(!seeded, "ended session must not be seeded");
        assert!(
            !map.cursors.contains_key("sessA"),
            "ended session must not be resurrected"
        );
    }

    #[test]
    fn sentinel_default_cursor_does_not_require_frame_ticks() {
        // Regression for idle CPU: a freshly-started serve daemon seeds only the
        // off-screen default cursor. With no commands in flight, the render loop
        // should be able to block on rx.recv() instead of repainting at 60fps.
        let map = empty_map();
        assert!(!render_map_needs_frame_tick(&map));
    }

    #[test]
    fn active_or_fading_cursor_requires_frame_ticks() {
        let mut map = empty_map();
        seed_start_in_map(&mut map, &"sessA".to_owned(), 60.0, 60.0);
        apply_msg(&mut map, move_msg("sessA", 80.0, 80.0));
        assert!(
            render_map_needs_frame_tick(&map),
            "planned path should tick"
        );

        let rs = map.cursors.get_mut("sessA").unwrap();
        rs.core.path = None;
        rs.core.spring = None;
        rs.core.click_t = None;
        rs.focus_rect = None;
        rs.core.idle_alpha = 0.0;
        assert!(
            !render_map_needs_frame_tick(&map),
            "fully hidden idle cursor should quiesce"
        );
    }

    #[test]
    fn visible_idle_dwell_sleeps_until_fade_start() {
        let mut map = empty_map();
        seed_start_in_map(&mut map, &"sessA".to_owned(), 60.0, 60.0);

        {
            let rs = map.cursors.get_mut("sessA").unwrap();
            rs.core.path = None;
            rs.core.spring = None;
            rs.core.click_t = None;
            rs.focus_rect = None;
            rs.core.visible = true;
            rs.core.pos = (60.0, 60.0);
            rs.core.motion.idle_hide_ms = 20_000.0;
            rs.core.idle_secs = 5.0;
            rs.core.idle_alpha = 1.0;
        }

        assert!(
            !render_map_needs_frame_tick(&map),
            "fully visible dwell should not composite at frame cadence"
        );
        let due = render_map_idle_fade_due_in(&map).expect("dwell should arm a fade timeout");
        assert!(
            (due.as_secs_f64() - 15.0).abs() < 0.001,
            "remaining dwell should be 15s, got {due:?}"
        );

        let rs = map.cursors.get_mut("sessA").unwrap();
        assert!(
            !rs.tick(due.as_secs_f64() + 0.01),
            "dwell sleep must not fire an arrival signal"
        );
        assert!(
            rs.core.idle_alpha < 1.0 && rs.core.idle_alpha >= 0.004,
            "full slept dwell duration should start the fade, alpha={}",
            rs.core.idle_alpha
        );
        assert!(
            render_map_needs_frame_tick(&map),
            "fade in progress should resume frame ticks"
        );
    }

    #[test]
    fn global_appkit_transform_preserves_left_and_above_main_coordinates() {
        let main = ScreenGeometry {
            origin_x: 10.0,
            origin_y: 20.0,
            height: 900.0,
            fallback_backing_scale: 2.0,
        };
        let left = appkit_frame_for_rect(
            LogicalRect {
                left: -1800.0,
                top: 100.0,
                width: 144.0,
                height: 144.0,
            },
            main,
        );
        assert_eq!(left.x, -1790.0);
        assert_eq!(left.y, 676.0);

        let above = appkit_frame_for_rect(
            LogicalRect {
                left: 300.0,
                top: -600.0,
                width: 144.0,
                height: 144.0,
            },
            main,
        );
        assert_eq!(above.x, 310.0);
        assert_eq!(above.y, 1376.0);
        assert_eq!(above.width, 144.0);
        assert_eq!(above.height, 144.0);
    }

    #[test]
    fn display_scale_resolution_handles_negative_above_and_pinned_targets() {
        let displays = [
            DisplayGeometry {
                bounds: LogicalRect {
                    left: -1920.0,
                    top: 0.0,
                    width: 1920.0,
                    height: 1080.0,
                },
                backing_scale: 1.0,
            },
            DisplayGeometry {
                bounds: LogicalRect {
                    left: 0.0,
                    top: 0.0,
                    width: 1440.0,
                    height: 900.0,
                },
                backing_scale: 2.0,
            },
            DisplayGeometry {
                bounds: LogicalRect {
                    left: 0.0,
                    top: -1200.0,
                    width: 1920.0,
                    height: 1200.0,
                },
                backing_scale: 1.5,
            },
        ];

        assert_eq!(
            backing_scale_for_cursor(&displays, 2.0, (-500.0, 300.0), None),
            1.0
        );
        assert_eq!(
            backing_scale_for_cursor(&displays, 2.0, (500.0, -300.0), None),
            1.5
        );
        assert_eq!(
            backing_scale_for_cursor(&displays, 1.0, (500.0, 300.0), None),
            2.0
        );

        let pinned_left = LogicalRect {
            left: -1700.0,
            top: 100.0,
            width: 1000.0,
            height: 700.0,
        };
        assert_eq!(
            backing_scale_for_cursor(&displays, 2.0, (500.0, 300.0), Some(pinned_left)),
            1.0,
            "a pinned panel should follow the target window's display"
        );

        let equal_split = LogicalRect {
            left: -100.0,
            top: 100.0,
            width: 200.0,
            height: 200.0,
        };
        assert_eq!(
            backing_scale_for_cursor(&displays, 1.0, (20.0, 150.0), Some(equal_split)),
            2.0,
            "cursor display should break a pinned-window intersection tie"
        );
    }

    #[test]
    fn backing_scale_transition_forces_native_rerasterization() {
        let displays = [
            DisplayGeometry {
                bounds: LogicalRect {
                    left: -1000.0,
                    top: 0.0,
                    width: 1000.0,
                    height: 800.0,
                },
                backing_scale: 1.0,
            },
            DisplayGeometry {
                bounds: LogicalRect {
                    left: 0.0,
                    top: 0.0,
                    width: 1000.0,
                    height: 800.0,
                },
                backing_scale: 2.0,
            },
        ];
        let one_x = backing_scale_for_cursor(&displays, 2.0, (-1.0, 400.0), None);
        let two_x = backing_scale_for_cursor(&displays, 1.0, (0.0, 400.0), None);
        assert_eq!((one_x, two_x), (1.0, 2.0));

        let mut rs = RenderState::new(CursorConfig::default());
        rs.core.visible = true;
        rs.core.pos = (60.0, 60.0);
        rs.core.idle_alpha = 1.0;

        let rect = cursor_window_rect(&rs).unwrap();
        let appearance = appearance_signature(&rs);
        let one_x_size = rect.pixel_size(one_x);
        let two_x_size = rect.pixel_size(two_x);
        assert_eq!(two_x_size, (one_x_size.0 * 2, one_x_size.1 * 2));
        let two_x_pixmap = render_cursor_pixmap(&rs, rect, two_x);
        assert_eq!((two_x_pixmap.width(), two_x_pixmap.height()), two_x_size);
        assert!(cursor_redraw_needed(
            Some(appearance),
            Some(one_x_size),
            Some(one_x),
            appearance,
            one_x_size,
            two_x,
            false,
            false,
        ));
        assert!(backing_scale_changed(Some(one_x), two_x));
        assert!(!backing_scale_changed(Some(two_x), two_x));
    }

    #[test]
    fn position_only_motion_moves_window_without_redrawing_bitmap() {
        let mut rs = RenderState::new(CursorConfig::default());
        rs.core.visible = true;
        rs.core.pos = (60.0, 60.0);
        rs.core.idle_alpha = 1.0;
        rs.core.heading = std::f64::consts::FRAC_PI_4;

        let rect_a = cursor_window_rect(&rs).unwrap();
        let app_a = appearance_signature(&rs);
        let size_a = rect_a.pixel_size(2.0);

        rs.core.pos = (90.0, 75.0);
        let rect_b = cursor_window_rect(&rs).unwrap();
        let app_b = appearance_signature(&rs);
        let size_b = rect_b.pixel_size(2.0);

        assert_ne!(
            appkit_frame_for_rect(
                rect_a,
                ScreenGeometry {
                    origin_x: 0.0,
                    origin_y: 0.0,
                    height: 200.0,
                    fallback_backing_scale: 2.0,
                }
            )
            .x,
            appkit_frame_for_rect(
                rect_b,
                ScreenGeometry {
                    origin_x: 0.0,
                    origin_y: 0.0,
                    height: 200.0,
                    fallback_backing_scale: 2.0,
                }
            )
            .x,
            "window frame should move when the cursor position changes"
        );
        assert_eq!(size_a, size_b, "cursor-only canvas size should stay fixed");
        assert!(
            !cursor_redraw_needed(
                Some(app_a),
                Some(size_a),
                Some(2.0),
                app_b,
                size_b,
                2.0,
                false,
                false
            ),
            "position-only movement should not redraw the small bitmap"
        );

        rs.core.heading += 0.1;
        let app_c = appearance_signature(&rs);
        assert!(
            cursor_redraw_needed(
                Some(app_b),
                Some(size_b),
                Some(2.0),
                app_c,
                size_b,
                2.0,
                false,
                false
            ),
            "heading changes must redraw rotation frames"
        );

        let mut sky_cfg = CursorConfig::default();
        sky_cfg.builtin_shape = cursor_overlay::BuiltinShape::Sky;
        let mut sky = RenderState::new(sky_cfg);
        sky.core.visible = true;
        sky.core.pos = (60.0, 60.0);
        sky.core.idle_alpha = 1.0;
        sky.core.heading = 0.0;
        let sky_rect = cursor_window_rect(&sky).unwrap();
        let sky_size = sky_rect.pixel_size(2.0);
        let sky_app_a = appearance_signature(&sky);
        sky.core.heading = std::f64::consts::PI;
        let sky_app_b = appearance_signature(&sky);
        assert!(
            !cursor_redraw_needed(
                Some(sky_app_a),
                Some(sky_size),
                Some(2.0),
                sky_app_b,
                sky_size,
                2.0,
                false,
                false
            ),
            "Sky ignores heading, so mid-glide heading changes must not redraw"
        );
    }

    #[test]
    fn panel_level_follows_pin_state() {
        assert_eq!(panel_level_for_pin(None), PanelLevel::Floating);
        assert_eq!(panel_level_for_pin(Some(42)), PanelLevel::TargetWindow);
        assert_eq!(PanelLevel::Floating.ns_window_level(), 3);
        assert_eq!(PanelLevel::TargetWindow.ns_window_level(), 0);
    }

    #[test]
    fn last_active_arrival_dwell_is_not_shortened_by_stale_cursor() {
        let mut map = empty_map();
        {
            let default = map.cursors.get_mut("default").unwrap();
            default.core.visible = true;
            default.core.pos = (20.0, 20.0);
            default.core.motion.idle_hide_ms = 20_000.0;
            default.core.idle_secs = 18.0;
            default.core.idle_alpha = 1.0;
        }
        assert!(
            (map.cursors["default"]
                .idle_fade_due_in()
                .unwrap()
                .as_secs_f64()
                - 2.0)
                .abs()
                < 0.001,
            "test setup must make the default cursor stale"
        );

        seed_start_in_map(&mut map, &"sessA".to_owned(), 60.0, 60.0);
        apply_msg(&mut map, move_msg("sessA", 80.0, 80.0));
        {
            let sess = map.cursors.get_mut("sessA").unwrap();
            // Simulate the just-arrived rest state through the same render-map
            // seam that arms recv_timeout(): no animations, fully visible,
            // idle accounting freshly reset by MoveTo.
            sess.core.path = None;
            sess.core.spring = None;
            sess.core.click_t = None;
            sess.focus_rect = None;
            sess.core.visible = true;
            sess.core.pos = (80.0, 80.0);
            sess.core.motion.idle_hide_ms = 20_000.0;
            sess.core.idle_secs = 0.0;
            sess.core.idle_alpha = 1.0;
        }

        assert_eq!(map.last_active_key.as_deref(), Some("sessA"));
        let due = render_map_idle_fade_due_in(&map).expect("fresh arrival should arm dwell");
        assert!(
            (due.as_secs_f64() - 20.0).abs() < 0.001,
            "fresh arrival must dwell for the full idle_hide_ms despite stale default, got {due:?}"
        );

        let sess = map.cursors.get_mut("sessA").unwrap();
        assert!(!sess.tick(19.99));
        assert_eq!(
            sess.core.idle_alpha, 1.0,
            "no tick sequence before fade_start may fade the fresh cursor"
        );
        assert!(!sess.tick(0.02));
        assert!(
            sess.core.idle_alpha < 1.0,
            "fade should still start once the full dwell has elapsed"
        );
    }

    #[test]
    fn frame_budget_tracks_screen_refresh_rate() {
        assert!((frame_budget_for_max_fps(120).as_secs_f64() - (1.0 / 120.0)).abs() < 0.0001);
        assert!((frame_budget_for_max_fps(60).as_secs_f64() - (1.0 / 60.0)).abs() < 0.0001);
        assert!((frame_budget_for_max_fps(0).as_secs_f64() - (1.0 / 60.0)).abs() < 0.0001);
        assert_eq!(repin_frame_interval(frame_budget_for_max_fps(120)), 120);
    }

    #[test]
    fn per_key_arrival_isolation() {
        // Two concurrent waiters keyed A and B; firing A must not cancel B.
        // This mirrors the ARRIVAL_TX HashMap logic in isolation (no statics).
        let mut waiters: HashMap<CursorKey, tokio::sync::oneshot::Sender<()>> = HashMap::new();
        let (txa, mut rxa) = tokio::sync::oneshot::channel::<()>();
        let (txb, mut rxb) = tokio::sync::oneshot::channel::<()>();
        waiters.insert("A".to_owned(), txa);
        waiters.insert("B".to_owned(), txb);

        // Fire A's arrival.
        if let Some(tx) = waiters.remove("A") {
            let _ = tx.send(());
        }
        // A resolved, B still pending.
        assert!(matches!(rxa.try_recv(), Ok(())));
        assert!(matches!(
            rxb.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
    }
}
