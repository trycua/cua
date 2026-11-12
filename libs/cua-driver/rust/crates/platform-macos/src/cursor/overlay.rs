// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

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
//! The render loop uses a GCD background thread at ~60 fps.  Each tick it
//! renders the animation state into a `tiny_skia::Pixmap`, converts to a
//! `CGImage`, and dispatches `CALayer.setContents` back to the main queue.
//!
//! ## Coordinate system
//!
//! All coordinates are **screen points** with the **top-left origin**
//! (matching `OverlayCommand::MoveTo` and AX element coordinates).  The
//! Each display has a surface with its own backing scale. Global points are
//! translated to display-local tiles; AppKit frames use a bottom-left origin.
//!
//! ## Cross-platform note (2026-05 dedup audit)
//!
//! Animation state + render pipeline live in `cursor_overlay::render_state`
//! (`RenderStateCore`, `tick_motion`, `apply_command_base`,
//! `render_frame`).  macOS uses the hardcoded Swift reference constants
//! (peakSpeed=900, springK=400, overshoot=0.8) and the sentinel-snap
//! variants of MoveTo / ClickPulse — see the wrapper around
//! `apply_command_base` below.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cursor_overlay::{
    CursorConfig, CursorKey, KeyedOverlayCommand, MotionConfig, MsgOutcome, OverlayCommand,
    OverlayMsg, RenderEntry, RenderStateCore, ScreenFrame, SurfaceGeometry, ZOrderEnforcer,
};

// ── Arrival-signal channels (one waiter slot per cursor key) ──────────────
//
// Each session's `animate_cursor_to` registers an arrival oneshot keyed by its
// own cursor key. A new animation only supersedes the SAME key's prior waiter,
// so concurrent sessions never cross-cancel each other's arrivals.

static ARRIVAL_TX: Mutex<Option<HashMap<CursorKey, tokio::sync::oneshot::Sender<()>>>> =
    Mutex::new(None);

fn arrival_register(key: CursorKey, tx: tokio::sync::oneshot::Sender<()>) {
    let mut guard = ARRIVAL_TX.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    // Cancel only the same key's previous waiter (superseded by new animation).
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

/// Drop a removed session's waiter; the dropped sender releases its await.
fn arrival_cancel(key: &CursorKey) {
    if let Ok(mut guard) = ARRIVAL_TX.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(key);
        }
    }
}

// ── Global overlay state ──────────────────────────────────────────────────

static CMD_TX: OnceLock<std::sync::mpsc::SyncSender<OverlayMsg>> = OnceLock::new();
// Single-consumer slot; receiver is moved into run_on_main_thread().
static CMD_RX_CELL: Mutex<Option<std::sync::mpsc::Receiver<OverlayMsg>>> = Mutex::new(None);
static RENDER: Mutex<Option<RenderMap>> = Mutex::new(None);
static OVERLAY_WINDOW_IDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static DISPLAY_LAYOUT: Mutex<Option<super::display_layout::DisplayLayout>> = Mutex::new(None);
/// True while the render loop that fires arrival signals runs. Only
/// [`run_on_main_thread`] (through `run_appkit`) starts it: a host that
/// initializes the overlay but never hands it the main thread has no one to
/// fire an arrival, so [`animate_cursor_to`] must not wait for one.
static RENDER_LOOP_RUNNING: AtomicBool = AtomicBool::new(false);
/// The longest [`animate_cursor_to`] waits for an arrival. A glide across the
/// largest display at the reference peak speed (900 pt/s) plus the spring
/// settles well inside it; a stalled render loop must never wedge a tool.
const ARRIVAL_TIMEOUT: Duration = Duration::from_secs(8);

pub(crate) fn is_overlay_window(window_id: u32) -> bool {
    window_id != 0 && OVERLAY_WINDOW_IDS.lock().unwrap().contains(&window_id)
}

/// Main-display overlay, for the main-display capture exclusion filter.
pub(crate) fn overlay_window_id() -> Option<u32> {
    OVERLAY_WINDOW_IDS.lock().unwrap().first().copied()
}

/// Whether the last composed frame could hold visible pixels, and when that
/// was last true. The layer update reaches WindowServer asynchronously, so a
/// frame that just cleared is still treated as visible for a short grace.
static OVERLAY_FRAME_PAINTS: AtomicBool = AtomicBool::new(false);
static OVERLAY_LAST_PAINT: Mutex<Option<Instant>> = Mutex::new(None);
const OVERLAY_CLEAR_GRACE: Duration = Duration::from_millis(250);
/// Cursor art and click effects fit within this many points of the anchor.
const CURSOR_TILE_MARGIN: f64 = 64.0;
/// The session badge's glow reaches this far past its pill.
const BADGE_GLOW_PAD: f64 = cursor_overlay::session_badge_extents().above as f64;

fn note_overlay_frame(paints: bool) {
    OVERLAY_FRAME_PAINTS.store(paints, Ordering::Release);
    if paints {
        if let Ok(mut last) = OVERLAY_LAST_PAINT.lock() {
            *last = Some(Instant::now());
        }
    }
}

/// Whether overlay pixels may be on screen right now: a cursor, its session
/// pill, a fade, or a focus rect. Desktop captures only need to exclude the
/// overlay window when this is true.
pub(crate) fn overlay_may_show_pixels() -> bool {
    if OVERLAY_FRAME_PAINTS.load(Ordering::Acquire) {
        return true;
    }
    OVERLAY_LAST_PAINT
        .lock()
        .ok()
        .and_then(|last| *last)
        .is_some_and(|last| last.elapsed() < OVERLAY_CLEAR_GRACE)
}

/// Negative coordinates are ordinary display positions, never hidden sentinels.
fn contains(f: ScreenFrame, (x, y): (f64, f64)) -> bool {
    x >= f.x && x < f.x + f.width && y >= f.y && y < f.y + f.height
}

#[cfg(test)]
fn paint_main_screen(pm: &mut tiny_skia::Pixmap, map: &RenderMap, origin: (f64, f64), scale: f32) {
    paint_surface(pm, map, origin, scale, main_geometry(map).frame);
}

fn finite_position(pos: (f64, f64)) -> bool {
    pos.0.is_finite() && pos.1.is_finite()
}

fn cursor_may_paint(state: &RenderState) -> bool {
    state.core.cfg.enabled
        && state.core.visible
        && state.core.placed
        && state.core.idle_alpha > 0.0
        && finite_position(state.core.pos)
}

/// Paint cursor art into a display tile at a global top-left origin.
/// Only the display containing the anchor paints the session badge.
fn paint_surface(
    pm: &mut tiny_skia::Pixmap,
    map: &RenderMap,
    origin: (f64, f64),
    backing_scale: f32,
    frame: ScreenFrame,
) {
    for rs in map.cursors.values() {
        if !finite_position(rs.core.pos) {
            continue;
        }
        cursor_overlay::render_state::paint_cursor_with_badge(
            pm,
            &rs.core,
            origin.0,
            origin.1,
            None,
            backing_scale,
            contains(frame, rs.core.pos),
        );
    }
}

/// The global region `[left, top, right, bottom]`, in whole points, that
/// this frame paints: each drawn cursor's tile grown by its motion effects
/// and focus rect, clamped to the screen. A custom theme may paint past the
/// tile, so a drawn cursor with one takes the whole screen. `None` when
/// nothing paints.
#[cfg(test)]
fn paint_bounds(map: &RenderMap) -> Option<[f64; 4]> {
    paint_bounds_for(map, main_geometry(map))
}

#[cfg(test)]
fn main_geometry(map: &RenderMap) -> SurfaceGeometry {
    SurfaceGeometry::new(
        ScreenFrame::new(0.0, 0.0, map.platform.win_w, map.platform.win_h),
        map.platform.backing_scale,
    )
}

fn paint_bounds_for(map: &RenderMap, geometry: SurfaceGeometry) -> Option<[f64; 4]> {
    let mut bounds: Option<[f64; 4]> = None;
    let mut add = |[x0, y0, x1, y1]: [f64; 4]| {
        if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
            return;
        }
        bounds = Some(match bounds {
            None => [x0, y0, x1, y1],
            Some([a, b, c, d]) => [a.min(x0), b.min(y0), c.max(x1), d.max(y1)],
        });
    };
    let f = geometry.frame;
    let (ox, oy, w, h) = (f.x, f.y, f.width, f.height);
    let scale = geometry.scale.max(1.0);
    for rs in map.cursors.values() {
        let core = &rs.core;
        if !finite_position(core.pos) || !core.is_revealed() || core.pinned_target_off_workspace {
            continue;
        }
        if core
            .theme
            .as_ref()
            .is_some_and(|theme| theme.id != cursor_overlay::DEFAULT_THEME_ID)
        {
            add([ox, oy, ox + w, oy + h]);
        }
        add([
            core.pos.0 - CURSOR_TILE_MARGIN,
            core.pos.1 - CURSOR_TILE_MARGIN,
            core.pos.0 + CURSOR_TILE_MARGIN,
            core.pos.1 + CURSOR_TILE_MARGIN,
        ]);
        // The badge is clamped into the screen, so near an edge it shifts
        // away from the cursor.
        if contains(f, core.pos) {
            if let Some(layout) = cursor_overlay::session_badge_layout(core.session_badge_input(
                (
                    ((core.pos.0 - ox) * scale) as f32,
                    ((core.pos.1 - oy) * scale) as f32,
                ),
                scale as f32,
                Some(((w * scale) as f32, (h * scale) as f32)),
            )) {
                let rect = layout.rect;
                add([
                    ox + f64::from(rect.left()) / scale - BADGE_GLOW_PAD,
                    oy + f64::from(rect.top()) / scale - BADGE_GLOW_PAD,
                    ox + f64::from(rect.right()) / scale + BADGE_GLOW_PAD,
                    oy + f64::from(rect.bottom()) / scale + BADGE_GLOW_PAD,
                ]);
            }
        }
        if let Some([x, y, w, h]) = core.effect_bounds() {
            add([x, y, x + w, y + h]);
        }
    }
    let [x0, y0, x1, y1] = bounds?;
    let left = x0.floor().clamp(ox, ox + w);
    let top = y0.floor().clamp(oy, oy + h);
    let right = x1.ceil().clamp(ox, ox + w);
    let bottom = y1.ceil().clamp(oy, oy + h);
    (right > left && bottom > top).then_some([left, top, right, bottom])
}

/// One frame's pixels and where they sit on their display.
struct PaintTile {
    /// `[left, top, width, height]` in display-local points.
    frame: [f64; 4],
    scale: f64,
    pixmap: tiny_skia::Pixmap,
}

/// Paint only the region this frame covers.
#[cfg(test)]
fn paint_tile(map: &RenderMap) -> Option<PaintTile> {
    paint_tile_for(map, main_geometry(map))
}

fn paint_tile_for(map: &RenderMap, geometry: SurfaceGeometry) -> Option<PaintTile> {
    let [left, top, right, bottom] = paint_bounds_for(map, geometry)?;
    let scale = geometry.scale.max(1.0);
    let mut pixmap = tiny_skia::Pixmap::new(
        ((right - left) * scale).ceil() as u32,
        ((bottom - top) * scale).ceil() as u32,
    )?;
    paint_surface(&mut pixmap, map, (left, top), scale as f32, geometry.frame);
    Some(PaintTile {
        frame: [
            left - geometry.frame.x,
            top - geometry.frame.y,
            right - left,
            bottom - top,
        ],
        scale,
        pixmap,
    })
}

/// Screen-global geometry kept beside the shared keyed render map
/// ([`cursor_overlay::RenderMap`], which owns the per-session lifecycle:
/// lazy creation, stable z-order, tombstones, revival, and the default guard).
/// Written in `run_appkit`, then refitted by the render loop whenever the
/// display geometry is supplied separately to each surface. These dimensions
/// are retained for the full-frame reference renderer in tests.
#[cfg_attr(not(test), allow(dead_code))]
struct MacScreen {
    win_w: f64,
    win_h: f64,
    /// `NSScreen.backingScaleFactor` of the screen the overlay window sits on.
    /// 1.0 on a non-retina display, 2.0 on a typical retina Mac. Drives the
    /// physical-pixel pixmap sizing + `paint_cursor` `backing_scale` so the
    /// rendered cursor is crisp at native resolution instead of being
    /// bilinear-upsampled by Core Animation from a logical-pixel buffer.
    backing_scale: f64,
}

type RenderMap = cursor_overlay::RenderMap<RenderState, MacScreen>;

/// Drain one message into the shared map, releasing a removed session's
/// arrival waiter. Returns the commanded key for z-order pinning.
fn apply_msg(map: &mut RenderMap, msg: OverlayMsg) -> Option<CursorKey> {
    let outcome = map.apply_msg(msg);
    if let MsgOutcome::Removed { key, .. } = &outcome {
        arrival_cancel(key);
    }
    outcome.applied_key().cloned()
}

/// Initialise global overlay state (call once, before run_on_main_thread).
pub fn init(cfg: CursorConfig) {
    static INITIALIZED: OnceLock<()> = OnceLock::new();
    INITIALIZED.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        CMD_TX
            .set(tx)
            .expect("cursor overlay sender is initialized exactly once");
        *CMD_RX_CELL.lock().unwrap() = Some(rx);
        *ARRIVAL_TX.lock().unwrap() = Some(HashMap::new());
        *RENDER.lock().unwrap() = Some(RenderMap::new(
            cfg,
            MacScreen {
                win_w: 0.0,
                win_h: 0.0,
                backing_scale: 1.0,
            },
        ));
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
                CursorEvent::KeyCombo { session, keys } => {
                    (session, OverlayCommand::ShowKeyCombo(keys))
                }
                CursorEvent::SelectMotion { session, motion } => {
                    (session, OverlayCommand::ApplyMotion(motion))
                }
            };
            send_command(session, cmd);
        },
    ));
}

/// Whether commands for `key` reach the renderer: not the empty no-cursor
/// key (direct platform calls that bypass lifecycle dispatch), and not a
/// human-origin session, whose client draws the human's own cursor (see
/// `cua_driver_core::agent_cursor`).
pub(crate) fn draws_cursor(key: &str) -> bool {
    !key.is_empty() && !cua_driver_core::agent_cursor::overlay_suppressed(key)
}

/// Send a keyed command from any thread (MCP tool, etc.).  Non-blocking; drops
/// if the channel is full (old commands are less important than new ones).
pub fn send_command(key: CursorKey, cmd: OverlayCommand) {
    if !draws_cursor(&key) {
        return;
    }
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(OverlayMsg::Cmd(KeyedOverlayCommand { key, cmd }));
    }
}

/// Convenience for callsites not yet threaded with a session key: drives the
/// seeded `"default"` cursor (the anonymous / one-shot identity).
pub fn send_command_default(cmd: OverlayCommand) {
    send_command("default".to_owned(), cmd);
}

/// Truthful render acknowledgement for lifecycle inspection. This never falls
/// back to the seeded default cursor: an absent, off-screen, disabled, or
/// idle-faded session cursor is not reported as visible.
pub fn is_visible_for_session(key: &str) -> bool {
    RENDER
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .and_then(|map| map.cursors.get(key))
                .map(cursor_is_externally_visible)
        })
        .unwrap_or(false)
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
        let _ = tx.try_send(OverlayMsg::Remove(key));
    }
}

/// Clear the render-side tombstone after a successful explicit session
/// revival. Cursor recreation remains lazy until the next render command.
pub fn revive_cursor(key: CursorKey) {
    if key.is_empty() {
        return;
    }
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(OverlayMsg::Revive(key));
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
    map.cursor_or_default(key)
        .map(|rs| rs.core.motion.clone())
        .unwrap_or_default()
}

/// Return the render-owned theme and semantic playback state for one cursor.
pub fn current_theme_state(
    key: &str,
) -> Option<(
    String,
    String,
    String,
    Option<String>,
    cursor_overlay::CursorVisualState,
)> {
    let guard = RENDER.lock().unwrap();
    let map = guard.as_ref()?;
    let state = map.cursor_or_default(key)?;
    let (id, version, profile, fallback) = state.core.active_theme_metadata();
    Some((id, version, profile, fallback, state.core.visual.clone()))
}

/// Seed a brand-new (sentinel-positioned) cursor at an on-screen start point
/// offset up-left of `(target_x, target_y)` so the immediately-following
/// `MoveTo` glides INTO the target instead of silently snapping. Without this,
/// a cursor's very first action (common on a pure-AX run — launch app, AX-press
/// a button) produces no visible motion: `animate_cursor_to` early-returned at
/// the sentinel and only `ClickPulse` snapped a static arrow, which is easy to
/// miss. See the AX-no-glide report.
///
/// No-op when the cursor is already placed or absent. Seed inside the
/// target display, including displays at negative global coordinates.
/// Returns true if a seed was applied (i.e. the cursor was at the sentinel and
/// is now primed to glide).
fn seed_start_if_unplaced(key: &CursorKey, target_x: f64, target_y: f64) -> bool {
    let mut guard = RENDER.lock().unwrap();
    let Some(map) = guard.as_mut() else {
        return false;
    };
    // `win_w/h` are 0 until the AppKit window is up; the shared seed then only
    // keeps the start point off negative coordinates.
    let frame = DISPLAY_LAYOUT
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|layout| layout.display_at(target_x, target_y))
        .map(|d| ScreenFrame::new(d.x, d.y, d.width, d.height));
    map.seed_start_if_unplaced(key, target_x, target_y, frame)
}

/// Animate the overlay cursor to `(x, y)` and suspend until the Dubins path
/// completes and the spring overshoot begins.
///
/// Mirrors Swift's `AgentCursor.shared.animateAndWait(to:)`.
/// Returns immediately (no animation) only when the overlay is disabled for
/// this cursor. A brand-new cursor still at the off-screen sentinel is first
/// seeded on-screen via [`seed_start_if_unplaced`] so its FIRST action glides
/// in (it previously snapped silently via `ClickPulse`, invisible on a pure-AX
/// run).
pub async fn animate_cursor_to(key: CursorKey, x: f64, y: f64) {
    animate_cursor_to_target(key, x, y, None).await;
}

/// [`animate_cursor_to`] with the targeted element's screen rect
/// `[x, y, width, height]` (same space as `x`/`y`), so motion styles can use
/// Fitts timing and highlight the target. `None` when the action has no
/// element (pixel coordinates).
pub async fn animate_cursor_to_target(key: CursorKey, x: f64, y: f64, target: Option<[f64; 4]>) {
    // Empty key is the explicit no-cursor sentinel → nothing to animate.
    if key.is_empty() {
        return;
    }
    // Seed a sentinel cursor on-screen so the MoveTo below glides instead of
    // being short-circuited. After this the cursor is placed, so the
    // should-animate check passes on the first action just like later ones.
    seed_start_if_unplaced(&key, x, y);

    // Check whether animation should run for THIS cursor. A disabled cursor
    // never animates; an absent cursor (seed found nothing to prime) is skipped.
    let should_animate = {
        let guard = RENDER.lock().unwrap();
        matches!(
            guard.as_ref().and_then(|m| m.cursors.get(&key)),
            Some(rs) if rs.core.cfg.enabled && rs.core.placed
        )
    };
    if !should_animate {
        return;
    }

    // Create a one-shot channel; store the sender (keyed) so the render thread
    // can fire it when this cursor's path finishes.
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    arrival_register(key.clone(), tx);

    // Send the MoveTo command (click offset applied inside apply_command).
    send_command(
        key,
        OverlayCommand::MoveTo {
            x,
            y,
            // Arrive pointing upper-left (45°), matching the macOS system-cursor
            // convention and Swift reference (`endAngleDegrees: 45`).
            end_heading_radians: std::f64::consts::FRAC_PI_4,
            target,
        },
    );

    // Await arrival signal (fired from render thread when Dubins path ends).
    // Without a render loop nothing ever fires it (a daemon whose main
    // thread never entered `run_on_main_thread`): the command stays queued
    // for a loop that starts later, and the action proceeds now. Bounded
    // either way.
    await_arrival(rx, RENDER_LOOP_RUNNING.load(Ordering::Acquire)).await;
}

/// Waits for the cursor's arrival signal for as long as the current step's
/// pacing allows. A step of a batch waits only briefly for the glide (it plays
/// on), so a chain of actions is not held up by one cursor move per step.
pub(crate) async fn await_arrival(
    rx: tokio::sync::oneshot::Receiver<()>,
    render_loop_running: bool,
) -> bool {
    let limit = cua_driver_core::step_pacing::glide_wait_limit(ARRIVAL_TIMEOUT);
    if limit < ARRIVAL_TIMEOUT {
        wait_for_arrival_briefly(rx, render_loop_running, limit).await
    } else {
        wait_for_arrival(rx, render_loop_running, ARRIVAL_TIMEOUT).await
    }
}

/// [`wait_for_arrival`] for a deliberately short wait: the cursor is still
/// gliding when the limit passes, which is expected and not logged.
async fn wait_for_arrival_briefly(
    rx: tokio::sync::oneshot::Receiver<()>,
    render_loop_running: bool,
    limit: Duration,
) -> bool {
    render_loop_running && tokio::time::timeout(limit, rx).await.is_ok()
}

/// Waits for `rx` only while a render loop runs, and never longer than
/// `limit`. Returns whether the arrival came.
async fn wait_for_arrival(
    rx: tokio::sync::oneshot::Receiver<()>,
    render_loop_running: bool,
    limit: Duration,
) -> bool {
    if !render_loop_running {
        return false;
    }
    match tokio::time::timeout(limit, rx).await {
        Ok(_) => true,
        Err(_) => {
            tracing::warn!("cursor overlay: no arrival within {limit:?}; continuing without it");
            false
        }
    }
}

/// Whether the overlay's render loop runs in this process (it fires the
/// arrivals [`animate_cursor_to`] waits for).
pub fn render_loop_running() -> bool {
    RENDER_LOOP_RUNNING.load(Ordering::Acquire)
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
// is the macOS-specific NSScreen window dimensions.

struct RenderState {
    core: RenderStateCore,
}

impl RenderEntry for RenderState {
    fn from_config(cfg: CursorConfig) -> Self {
        RenderState {
            core: RenderStateCore::new(cfg),
        }
    }

    fn core(&self) -> &RenderStateCore {
        &self.core
    }

    fn core_mut(&mut self) -> &mut RenderStateCore {
        &mut self.core
    }

    /// Advance the animation by `dt` with the shared trajectory player (see
    /// [`RenderStateCore::tick_motion`]). Returns true if an arrival signal
    /// should be fired (the cursor just reached its target).
    fn tick(&mut self, dt: f64) -> bool {
        self.core.tick_motion(dt)
    }

    fn apply_command(&mut self, cmd: OverlayCommand) -> bool {
        // macOS uses the sentinel-snap variants of MoveTo / ClickPulse:
        //   - MoveTo only snaps `self.pos` if the cursor is still at the
        //     off-screen sentinel `(-200, -200)` (otherwise the path starts
        //     from the current position so the animation is continuous).
        //   - ClickPulse only updates `self.pos` if the cursor is still at
        //     the sentinel (otherwise the animation already landed it there).
        match cmd {
            OverlayCommand::PinAbove(wid) => {
                // The overlay window joins every Space, so a target on another
                // Space would otherwise animate over the user's current one at
                // that window's coordinates. Unknown membership keeps painting.
                self.core.pinned_target_off_workspace = u32::try_from(wid)
                    .ok()
                    .and_then(crate::windows::window_on_current_space_by_id)
                    == Some(false);
                self.core.apply_command_base(cmd, true, true)
            }
            other => self.core.apply_command_base(other, true, true),
        }
    }

    /// The shared predicate (glide, spring, pulse, badge, resting motion,
    /// idle fade) plus one macOS term: the opaque idle-hide countdown. The macOS loop parks on a blocking `recv` with no
    /// deadline, so a reduced-motion cursor must keep frame ticks through the
    /// countdown for its idle fade to start on time.
    fn needs_frame_tick(&self) -> bool {
        self.core.needs_frame_tick()
            || (self.core.motion.idle_hide_ms > 0.0 && self.core.is_revealed())
    }
}

// ── AppKit / CGImage plumbing ─────────────────────────────────────────────

#[derive(Clone, Copy)]
struct DisplaySurface {
    geometry: SurfaceGeometry,
    win_ptr: usize,
    layer_ptr: usize,
    window_id: u32,
}

/// AppKit window creation and disposal are confined to its main queue.
unsafe fn create_surface(
    d: super::display_layout::DisplayGeometry,
    primary_height: f64,
) -> Option<DisplaySurface> {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    let screen_frame = d.appkit_frame(primary_height);
    // ---- NSWindow: single alloc + initWithContentRect:... ----
    let win: *mut AnyObject = {
        let allocated: *mut AnyObject = msg_send![class!(NSWindow), alloc];
        // NSWindowStyleMaskBorderless = 0
        // NSBackingStoreBuffered = 2
        let w: *mut AnyObject = msg_send![allocated,
            initWithContentRect: screen_frame
            styleMask: 0u64
            backing: 2u64
            defer: false
        ];
        w
    };
    if win.is_null() {
        return None;
    }

    let _: () = msg_send![win, setOpaque: false];
    let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
    let _: () = msg_send![win, setBackgroundColor: clear];
    let _: () = msg_send![win, setHasShadow: false];
    let _: () = msg_send![win, setIgnoresMouseEvents: true];
    // NSWindowSharingReadOnly = 1. AppKit documents this as the default, but
    // set it explicitly for the transparent agent overlay so ScreenCaptureKit
    // includes browser-session cursors in Cua Driver recordings. Tahoe can
    // otherwise show the overlay live while omitting it from an in-process
    // display recording.
    let _: () = msg_send![win, setSharingType: 1u64];
    // NSNormalWindowLevel = 0.  The overlay lives at the normal window level so
    // it appears in CGWindowList layer=0 results (which agents inspect via
    // list_windows).  Z-ordering above the target is managed dynamically via
    // orderWindow:relativeTo: (see dispatch_pin_above / render_loop repin).
    let _: () = msg_send![win, setLevel: 0i64];
    // NSWindowCollectionBehaviorCanJoinAllSpaces(1<<0) | FullScreenAuxiliary(1<<8) | Stationary(1<<4)
    let _: () = msg_send![win, setCollectionBehavior: (1u64 | (1<<8) | (1<<4))];
    let _: () = msg_send![win, setReleasedWhenClosed: false];
    let _: () = msg_send![win, setHidesOnDeactivate: false];

    // ---- Layer-backed content view ----
    let content_view: *mut AnyObject = msg_send![win, contentView];
    let _: () = msg_send![content_view, setWantsLayer: true];
    let layer: *mut AnyObject = msg_send![content_view, layer];

    let tile_layer: *mut AnyObject = msg_send![class!(CALayer), new];
    let _: () = msg_send![layer, addSublayer: tile_layer];

    let number: isize = msg_send![win, windowNumber];
    let _: () = msg_send![tile_layer, release]; // owned by the root layer
    let _: () = msg_send![win, orderFrontRegardless];
    Some(DisplaySurface {
        geometry: SurfaceGeometry::new(
            ScreenFrame::new(d.x, d.y, d.width, d.height),
            d.backing_scale,
        ),
        win_ptr: win as usize,
        layer_ptr: tile_layer as usize,
        window_id: u32::try_from(number).ok()?,
    })
}

fn on_appkit<T: Send, F: FnOnce() -> T + Send>(work: F) -> T {
    use std::ffi::c_void;
    #[link(name = "System", kind = "framework")]
    extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_sync_f(
            queue: *const c_void,
            context: *mut c_void,
            work: unsafe extern "C" fn(*mut c_void),
        );
    }
    unsafe extern "C" fn invoke<F: FnOnce() -> T, T>(ctx: *mut c_void) {
        let (work, result) = &mut *(ctx as *mut (Option<F>, Option<T>));
        *result = Some(work.take().unwrap()());
    }
    let mut context = (Some(work), None);
    unsafe {
        dispatch_sync_f(
            &raw const _dispatch_main_q as *const c_void,
            &mut context as *mut _ as *mut c_void,
            invoke::<F, T>,
        );
    }
    context.1.unwrap()
}

unsafe fn dispose_surface(surface: DisplaySurface) {
    use objc2::msg_send;
    let win = surface.win_ptr as *mut objc2::runtime::AnyObject;
    let _: () = msg_send![win, orderOut: std::ptr::null::<objc2::runtime::AnyObject>()];
    let _: () = msg_send![win, close];
    let _: () = msg_send![win, release];
}

unsafe fn rebuild_surfaces(
    layout: &super::display_layout::DisplayLayout,
    old: Vec<DisplaySurface>,
) -> Vec<DisplaySurface> {
    // Build the replacement before touching the live set. A transient display
    // reconfiguration or allocation failure must not remove working overlays.
    let mut surfaces = Vec::new();
    for d in &layout.displays {
        let Some(surface) = create_surface(*d, layout.primary_height().unwrap_or(0.0)) else {
            for surface in surfaces {
                dispose_surface(surface);
            }
            return old;
        };
        surfaces.push(surface);
    }
    // Main queue is FIFO: pending old-layer presentations finish before disposal.
    for surface in old {
        dispose_surface(surface);
    }
    *OVERLAY_WINDOW_IDS.lock().unwrap() = surfaces.iter().map(|s| s.window_id).collect();
    *DISPLAY_LAYOUT.lock().unwrap() = Some(layout.clone());
    surfaces
}

unsafe fn run_appkit(_cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayMsg>) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    // ---- NSApplication ----
    // Verify main thread (MainThreadMarker is a zero-size compile-time token).
    let _mtm = objc2_foundation::MainThreadMarker::new()
        .expect("run_appkit must be called from the main thread");

    // AppKit uses Text Input Sources on this thread from here on.
    crate::input::input_source::main_thread_runs_event_loop();
    let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
    // NSApplicationActivationPolicyAccessory = 1 (no Dock icon, no menu bar)
    // setActivationPolicy: returns BOOL (success), not void.
    let _: bool = msg_send![app, setActivationPolicy: 1i64];
    // Finish launching without presenting a UI (needed for NSApp.run())
    let _: () = msg_send![app, finishLaunching];

    let Ok(layout) = super::display_layout::active_layout(0) else {
        return;
    };
    let surfaces = rebuild_surfaces(&layout, Vec::new());
    RENDER_LOOP_RUNNING.store(true, Ordering::Release);
    std::thread::spawn(move || {
        render_loop(surfaces, layout, rx);
        RENDER_LOOP_RUNNING.store(false, Ordering::Release);
    });

    // ---- NSApplication run loop (blocks until process exits) ----
    let _: () = msg_send![app, run];
    OVERLAY_WINDOW_IDS.lock().unwrap().clear();
}

fn render_loop(
    mut surfaces: Vec<DisplaySurface>,
    mut layout: super::display_layout::DisplayLayout,
    rx: std::sync::mpsc::Receiver<OverlayMsg>,
) {
    let target_frame_ms = Duration::from_millis(16); // ~60 fps while pixels can change
    let hover_poll_ms = Duration::from_millis(80);
    let mut last_tick = Instant::now();
    let mut frame_tick_needed = false;
    let mut hover_poll_needed = false;
    // Repin bookkeeping: track last pinned wid and a frame counter for
    // the periodic defensive-repin (every ~60 active frames ≈ 1 s).
    let mut last_pinned: Option<u64> = None;
    let mut repin_frames: u32 = 0;
    let mut last_display_check = Instant::now();

    loop {
        // When no cursor animation/fade is active, block until the MCP side
        // sends a command. This is the idle-server fast path: no fullscreen
        // pixmap allocation, no CGImage conversion, no 60fps wakeup.
        let (first_msg, hover_poll_tick) = if frame_tick_needed {
            (None, hover_poll_needed)
        } else if hover_poll_needed {
            match rx.recv_timeout(hover_poll_ms) {
                Ok(msg) => (Some(msg), true),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (None, true),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(msg) => (Some(msg), false),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (None, false),
                Err(_) => break,
            }
        };

        let woke_from_idle = first_msg.is_some();
        let now = Instant::now();
        let dt = if woke_from_idle {
            // The blocking recv() above can span an arbitrarily long idle period.
            // Do not charge that time to the first animation tick after a command;
            // let the wake-up frame render the newly-applied state at t=0.
            0.0
        } else {
            now.duration_since(last_tick).as_secs_f64().min(0.05)
        };
        last_tick = now;

        let mut refit = false;
        if last_display_check.elapsed() >= Duration::from_millis(250) {
            last_display_check = now;
            if let Ok(reading) = super::display_layout::active_layout(layout.generation) {
                if surfaces.is_empty() || reading != layout {
                    surfaces = on_appkit(move || unsafe { rebuild_surfaces(&reading, surfaces) });
                    if let Some(applied) = DISPLAY_LAYOUT.lock().unwrap().clone() {
                        layout = applied;
                    }
                    refit = true;
                }
            }
        }

        // ── Phase 1: drain + tick all cursors (one lock acquisition) ──────
        // `pinned_wid` follows the most-recently-updated cursor: a single
        // NSWindow can occupy only one z-band, so the last-active cursor's
        // target wins. `arrived` collects the keys whose path just ended.
        let (
            pinned_wid,
            raise_unpinned,
            arrived,
            had_msg,
            hover_changed,
            next_frame_tick_needed,
            next_hover_poll_needed,
        ) = {
            let mut guard = RENDER.lock().unwrap();
            match guard.as_mut() {
                Some(map) => {
                    // Drain via get-or-create; track the last-touched key so we
                    // can read its pinned_wid after ticking.
                    let mut last_key: Option<CursorKey> = None;
                    let mut had_msg = false;
                    if let Some(msg) = first_msg {
                        had_msg = true;
                        if let Some(k) = apply_msg(map, msg) {
                            last_key = Some(k);
                        }
                    }
                    while let Ok(msg) = rx.try_recv() {
                        had_msg = true;
                        if let Some(k) = apply_msg(map, msg) {
                            last_key = Some(k);
                        }
                    }
                    // Tick every cursor while an animation/fade is in progress
                    // or immediately after a command changed render state. The
                    // latter lets a just-created path/click/focus rect start on
                    // this frame without waiting for the next 16ms tick.
                    let mut arrived: Vec<CursorKey> = Vec::new();
                    if frame_tick_needed || had_msg {
                        arrived = map.tick_all(dt);
                    }
                    let pointer = if hover_poll_tick
                        || map
                            .cursors
                            .values()
                            .any(|rs| rs.core.session_badge_needs_hover_poll())
                    {
                        hardware_cursor_position()
                    } else {
                        None
                    };
                    let mut hover_changed = false;
                    if pointer.is_some() || hover_poll_tick {
                        for rs in map.cursors.values_mut() {
                            hover_changed |= rs.core.update_session_badge_hover(pointer);
                        }
                    }
                    let pinned = last_key
                        .as_ref()
                        .and_then(|k| map.cursors.get(k))
                        .map(|rs| rs.core.pinned_wid)
                        .unwrap_or(last_pinned);
                    let raise_unpinned = last_key
                        .as_ref()
                        .and_then(|k| map.cursors.get(k))
                        .is_some_and(cursor_is_externally_visible)
                        && pinned.is_none();
                    let next_frame_tick_needed = map.needs_frame_tick();
                    let next_hover_poll_needed = map
                        .cursors
                        .values()
                        .any(|rs| rs.core.session_badge_needs_hover_poll());
                    (
                        pinned,
                        raise_unpinned,
                        arrived,
                        had_msg,
                        hover_changed,
                        next_frame_tick_needed,
                        next_hover_poll_needed,
                    )
                }
                None => break,
            }
        };

        // Fire arrival signals so each session's animate_cursor_to() unblocks.
        for k in &arrived {
            arrival_fire(k);
        }

        // Repin: immediately on target change, then defensive every ~1 s while
        // the render loop is active. When quiescent, z-order is left unchanged
        // until the next command wakes the loop.
        if frame_tick_needed || had_msg {
            repin_frames += 1;
            let pin_changed = pinned_wid != last_pinned;
            last_pinned = pinned_wid;
            if pinned_wid.is_some() && (pin_changed || refit || repin_frames >= 60) {
                for surface in &surfaces {
                    MacZOrderEnforcer {
                        win_ptr: surface.win_ptr,
                    }
                    .reassert(pinned_wid);
                }
                repin_frames = 0;
            } else if raise_unpinned {
                // A direct move_cursor has no target window to pin against.
                // Raise the normal-level, click-through overlay without
                // activating the driver so a later foreground application
                // cannot cover a standalone session cursor.
                for surface in &surfaces {
                    dispatch_order_front(surface.win_ptr);
                }
                repin_frames = 0;
            } else if repin_frames >= 60 {
                repin_frames = 0;
            }
        }

        // ── Phase 2: composite one tile per display ─────────────────
        // Render only when a command arrived or the previous/next tick can
        // change pixels. A final frame is emitted as animations/fades finish so
        // the layer is left in the completed/cleared state before blocking.
        if had_msg || refit || hover_changed || frame_tick_needed || next_frame_tick_needed {
            let tiles = {
                let guard = RENDER.lock().unwrap();
                let Some(map) = guard.as_ref() else {
                    break;
                };
                note_overlay_frame(map.cursors.values().any(cursor_may_paint));
                surfaces
                    .iter()
                    .map(|surface| (surface.layer_ptr, paint_tile_for(map, surface.geometry)))
                    .collect::<Vec<_>>()
            };
            for (layer, tile) in tiles {
                dispatch_set_layer_contents(layer, tile);
            }
        }

        frame_tick_needed = next_frame_tick_needed;
        hover_poll_needed = next_hover_poll_needed;
        if frame_tick_needed {
            // Sleep remainder of frame budget.
            let elapsed = Instant::now().duration_since(last_tick);
            if let Some(remaining) = target_frame_ms.checked_sub(elapsed) {
                std::thread::sleep(remaining);
            }
        }
    }
}

fn hardware_cursor_position() -> Option<(f64, f64)> {
    use core_graphics::{
        event::CGEvent,
        event_source::{CGEventSource, CGEventSourceStateID},
    };

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
    let event = CGEvent::new(source).ok()?;
    let location = event.location();
    Some((location.x, location.y))
}

fn cursor_is_externally_visible(state: &RenderState) -> bool {
    state.core.cfg.enabled
        && state.core.visible
        && state.core.placed
        && finite_position(state.core.pos)
        && state.core.idle_alpha >= 0.004
}

/// Place `tile` on the tile `CALayer` via `dispatch_async(main_queue, ...)`;
/// `None` clears it.
fn dispatch_set_layer_contents(layer_ptr: usize, tile: Option<PaintTile>) {
    let (cg_image_ptr, frame, scale) = match tile {
        Some(tile) => match pixmap_to_cgimage(&tile.pixmap) {
            Some(p) => (p, tile.frame, tile.scale),
            None => return,
        },
        None => (0, [0.0; 4], 1.0),
    };

    // Box the payload for the C callback.
    let payload = Box::new((layer_ptr, cg_image_ptr, frame, scale));

    // GCD symbols from libdispatch (part of the macOS system library stubs).
    // `dispatch_get_main_queue()` is an inline C function; the underlying
    // symbol is `_dispatch_main_q`, a *struct* (not a pointer).
    // We declare it as `u8` (opaque placeholder) and take its ADDRESS to
    // obtain the `dispatch_queue_t` (pointer to the struct).
    #[link(name = "System", kind = "framework")]
    extern "C" {
        // Opaque placeholder — we only ever take &_dispatch_main_q, never read it.
        static _dispatch_main_q: u8;
        fn dispatch_async_f(
            queue: *const c_void,
            context: *mut c_void,
            work: unsafe extern "C" fn(*mut c_void),
        );
    }

    unsafe extern "C" fn set_contents_cb(ctx: *mut c_void) {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        let (layer_ptr, cg_image_ptr, [x, top, w, h], scale): (usize, usize, [f64; 4], f64) =
            *Box::from_raw(ctx as *mut _);
        let layer = layer_ptr as *mut AnyObject;
        // A standalone layer animates frame and contents changes by default.
        let _: () = msg_send![class!(CATransaction), begin];
        let _: () = msg_send![class!(CATransaction), setDisableActions: true];
        if cg_image_ptr != 0 {
            let root: *mut AnyObject = msg_send![layer, superlayer];
            let flipped: bool = msg_send![root, isGeometryFlipped];
            let bounds: NSRect = msg_send![root, bounds];
            let y = if flipped {
                top
            } else {
                bounds.size.height - top - h
            };
            let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
            let _: () = msg_send![layer, setContentsScale: scale];
            let _: () = msg_send![layer, setFrame: frame];
        }
        // setContents: expects an `id` (type '@'), not a raw void pointer.
        // CGImageRef is toll-free bridged to NSObject, so we cast it to *mut AnyObject.
        let cg_id = cg_image_ptr as *mut AnyObject;
        let _: () = msg_send![layer, setContents: cg_id];
        let _: () = msg_send![class!(CATransaction), commit];
        if cg_image_ptr != 0 {
            // Release the CGImage ref we retained in pixmap_to_cgimage.
            CGImageRelease(cg_image_ptr as *mut c_void);
        }
    }

    extern "C" {
        fn CGImageRelease(image: *mut c_void);
    }

    unsafe {
        // &_dispatch_main_q is the queue pointer (same as dispatch_get_main_queue()).
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(
            main_queue,
            Box::into_raw(payload) as *mut c_void,
            set_contents_cb,
        );
    }
}

/// Raise the normal-level overlay without activating the driver application.
///
/// This is used only for an externally visible cursor with no target window.
/// Target-bound actions continue to use [`dispatch_pin_above`] so background
/// delivery remains below unrelated foreground applications.
fn dispatch_order_front(win_ptr: usize) {
    use std::ffi::c_void;

    #[link(name = "System", kind = "framework")]
    extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_async_f(
            queue: *const c_void,
            context: *mut c_void,
            work: unsafe extern "C" fn(*mut c_void),
        );
    }

    unsafe extern "C" fn order_front_cb(ctx: *mut c_void) {
        let win_ptr = *Box::from_raw(ctx as *mut usize);
        let win = win_ptr as *mut objc2::runtime::AnyObject;
        let _: () = objc2::msg_send![win, orderFrontRegardless];
    }

    let payload = Box::new(win_ptr);
    unsafe {
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(
            main_queue,
            Box::into_raw(payload) as *mut c_void,
            order_front_cb,
        );
    }
}

/// Order the overlay NSWindow just above `target_wid` in the global window
/// server list.  Called from the render thread; dispatches to the main queue
/// (AppKit must be used on the main thread).
///
/// `NSWindowAbove = 1`; `orderWindow:relativeTo:` accepts any CGWindowID as
/// the `relativeTo` argument — it works cross-application via CGS.
fn target_is_frontmost_visible_window(
    target_wid: u64,
    frontmost_pid: Option<i32>,
    windows: &[crate::windows::WindowInfo],
) -> bool {
    let Some(target) = windows
        .iter()
        .find(|window| u64::from(window.window_id) == target_wid)
    else {
        return false;
    };
    if !target.is_on_screen || target.layer != 0 || frontmost_pid != Some(target.pid) {
        return false;
    }

    windows
        .iter()
        .filter(|window| {
            window.is_on_screen
                && window.layer == 0
                && window.pid == target.pid
                && window.bounds.width > 1.0
                && window.bounds.height > 1.0
        })
        .max_by_key(|window| window.z_index)
        .is_some_and(|window| u64::from(window.window_id) == target_wid)
}

fn dispatch_pin_above(win_ptr: usize, target_wid: u64) {
    use std::ffi::c_void;

    #[link(name = "System", kind = "framework")]
    extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_async_f(
            queue: *const c_void,
            context: *mut c_void,
            work: unsafe extern "C" fn(*mut c_void),
        );
    }

    unsafe extern "C" fn reorder_cb(ctx: *mut c_void) {
        let (win_ptr, target_wid, raise_front): (usize, u64, bool) =
            *Box::from_raw(ctx as *mut (usize, u64, bool));
        let win = win_ptr as *mut objc2::runtime::AnyObject;
        // NSWindowAbove = 1; relativeTo: takes NSInteger (i64 on 64-bit)
        let _: () = objc2::msg_send![win, orderWindow: 1i64 relativeTo: target_wid as i64];

        // Tahoe can leave a normal-level transparent window behind an
        // already-frontmost cross-process target even after the relative
        // ordering request. `orderFrontRegardless` does not activate the
        // driver app. Use it only when the exact target is already the
        // frontmost visible normal window, so background browser actions do
        // not put the overlay above the user's foreground app.
        if raise_front {
            let _: () = objc2::msg_send![win, orderFrontRegardless];
        }
    }

    let windows = crate::windows::visible_windows();
    let raise_front =
        target_is_frontmost_visible_window(target_wid, crate::apps::frontmost_pid(), &windows);
    let payload = Box::new((win_ptr, target_wid, raise_front));
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
/// `target = None` is treated as a no-op here. Direct unpinned cursor commands
/// raise the overlay once in the render loop, while this enforcer remains
/// responsible only for target-relative ordering.
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

    /// A human-origin session (a Cua Spaces viewer's relayed input) never
    /// reaches the renderer; an agent's session does. The rule itself lives
    /// in `cua_driver_core::agent_cursor`.
    #[test]
    fn human_origin_sessions_draw_no_agent_cursor() {
        use cua_driver_core::agent_cursor::{set_input_origin, InputOrigin};
        let human = "overlay-test-human-origin-session";
        assert!(draws_cursor(human));
        set_input_origin(human, InputOrigin::Human);
        assert!(!draws_cursor(human));
        assert!(draws_cursor("overlay-test-agent-session"));
        assert!(!draws_cursor(""));
        set_input_origin(human, InputOrigin::Agent);
        assert!(draws_cursor(human));
    }
    use std::collections::HashMap;

    // The keyed lifecycle, sentinel seed, and shared frame-tick predicate are
    // covered once in `cursor_overlay::render_map`. These tests cover only the
    // macOS adapter: window ordering, external visibility, and the macOS
    // frame-tick terms layered on the shared predicate.

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
    }

    /// cua-spacesd `serve` initialized the overlay (a GUI session) but never
    /// ran its AppKit loop, and the first animated click waited forever for
    /// an arrival only the render loop fires, wedging the driver.
    #[test]
    fn animate_without_a_render_loop_never_waits() {
        init(CursorConfig {
            enabled: true,
            ..CursorConfig::default()
        });
        assert!(!render_loop_running());
        let done = runtime().block_on(async {
            tokio::time::timeout(
                Duration::from_secs(2),
                animate_cursor_to("session-no-loop".to_owned(), 240.0, 180.0),
            )
            .await
        });
        assert!(
            done.is_ok(),
            "animate_cursor_to waited for a render loop that is not running"
        );
    }

    #[test]
    fn arrival_wait_is_bounded_and_skipped_without_a_loop() {
        let rt = runtime();
        let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
        assert!(!rt.block_on(wait_for_arrival(rx, false, Duration::from_secs(60))));
        let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
        let started = Instant::now();
        assert!(!rt.block_on(wait_for_arrival(rx, true, Duration::from_millis(50))));
        assert!(started.elapsed() < Duration::from_secs(5));
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tx.send(()).unwrap();
        assert!(rt.block_on(wait_for_arrival(rx, true, Duration::from_secs(60))));
    }

    #[test]
    fn a_batched_step_waits_for_the_glide_only_up_to_the_batch_cap() {
        use cua_driver_core::step_pacing::{glide_wait_limit, scope, StepPacing, BATCH_GLIDE_WAIT};
        let rt = runtime();
        // The sender stays alive and never fires: the glide is still playing.
        let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
        let started = Instant::now();
        let arrived = rt.block_on(scope(StepPacing::Batched, async {
            let limit = glide_wait_limit(ARRIVAL_TIMEOUT);
            assert_eq!(limit, BATCH_GLIDE_WAIT);
            wait_for_arrival_briefly(rx, true, limit).await
        }));
        assert!(!arrived);
        assert!(started.elapsed() >= BATCH_GLIDE_WAIT);
        assert!(started.elapsed() < ARRIVAL_TIMEOUT / 4);
        // A direct call keeps the full arrival limit.
        assert_eq!(glide_wait_limit(ARRIVAL_TIMEOUT), ARRIVAL_TIMEOUT);
        // An arrival inside the cap is still seen.
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tx.send(()).unwrap();
        assert!(rt.block_on(wait_for_arrival_briefly(rx, true, BATCH_GLIDE_WAIT)));
    }

    fn window(window_id: u32, pid: i32, z_index: usize) -> crate::windows::WindowInfo {
        crate::windows::WindowInfo {
            window_id,
            pid,
            app_name: format!("app-{pid}"),
            title: String::new(),
            bounds: crate::windows::WindowBounds {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
            layer: 0,
            z_index,
            is_on_screen: true,
            current_space_id: None,
            on_current_space: None,
            space_ids: None,
        }
    }

    fn empty_map() -> RenderMap {
        RenderMap::new(
            CursorConfig::default(),
            MacScreen {
                win_w: 100.0,
                win_h: 100.0,
                backing_scale: 1.0,
            },
        )
    }

    fn placed<'a>(map: &'a mut RenderMap, key: &str) -> &'a mut RenderState {
        let frame = Some(ScreenFrame::new(0.0, 0.0, 100.0, 100.0));
        assert!(map.seed_start_if_unplaced(key, 60.0, 60.0, frame));
        map.cursors.get_mut(key).unwrap()
    }

    #[test]
    fn tiles_render_on_negative_and_lower_displays_at_each_backing_scale() {
        for (x, y) in [(-400.0, 0.0), (0.0, -300.0), (0.0, 820.0), (1180.0, 0.0)] {
            for scale in [1.0, 2.0] {
                let mut map = empty_map();
                placed(&mut map, "sessA").core.pos = (x + 150.0, y + 150.0);
                let geometry = SurfaceGeometry::new(ScreenFrame::new(x, y, 400.0, 300.0), scale);
                let tile = paint_tile_for(&map, geometry).expect("secondary display must paint");
                assert!(tile.pixmap.pixels().iter().any(|p| p.alpha() > 0));
                assert!(tile.frame[0] >= 0.0 && tile.frame[1] >= 0.0);
                assert!(tile.frame[0] + tile.frame[2] <= 400.0);
                assert_eq!(tile.pixmap.width(), (tile.frame[2] * scale).ceil() as u32);
                assert_eq!(tile.pixmap.height(), (tile.frame[3] * scale).ceil() as u32);
            }
        }
    }

    #[test]
    fn cross_display_art_paints_both_sides_and_clears_departed_display() {
        let mut map = empty_map();
        placed(&mut map, "sessA").core.pos = (-2.0, 50.0);
        let left = SurfaceGeometry::new(ScreenFrame::new(-400.0, 0.0, 400.0, 300.0), 2.0);
        let right = SurfaceGeometry::new(ScreenFrame::new(0.0, 0.0, 400.0, 300.0), 1.0);
        for geometry in [left, right] {
            let tile = paint_tile_for(&map, geometry).unwrap();
            assert!(
                tile.pixmap.pixels().iter().any(|p| p.alpha() > 0),
                "cursor crossing seam must not clip"
            );
        }
        map.cursors.get_mut("sessA").unwrap().core.pos = (200.0, 150.0);
        assert!(paint_tile_for(&map, left).is_none());
        assert!(paint_tile_for(&map, right).is_some());
    }

    #[test]
    fn frontmost_target_can_raise_overlay_without_covering_another_app() {
        let target_pid = 100;
        let mut windows = vec![window(10, target_pid, 20), window(11, 200, 10)];
        // WindowServer may retain another app's window ahead in its global
        // list; the active app identity is the authoritative cross-app guard.
        windows[1].z_index = 30;
        assert!(target_is_frontmost_visible_window(
            10,
            Some(target_pid),
            &windows,
        ));

        // A different foreground app blocks the fallback raise.
        assert!(!target_is_frontmost_visible_window(10, Some(200), &windows,));

        // So does another visible window belonging to the active target app.
        windows.push(window(13, target_pid, 40));
        assert!(!target_is_frontmost_visible_window(
            10,
            Some(target_pid),
            &windows,
        ));
    }

    #[test]
    fn only_enabled_on_screen_cursor_is_externally_visible() {
        let mut map = empty_map();
        assert!(!cursor_is_externally_visible(&map.cursors["default"]));

        placed(&mut map, "sessA");
        assert!(cursor_is_externally_visible(&map.cursors["sessA"]));
        map.cursors.get_mut("sessA").unwrap().core.placed = false;
        assert!(!cursor_may_paint(&map.cursors["sessA"]));
        assert!(!cursor_is_externally_visible(&map.cursors["sessA"]));
        map.cursors.get_mut("sessA").unwrap().core.placed = true;

        map.cursors.get_mut("sessA").unwrap().core.cfg.enabled = false;
        assert!(!cursor_is_externally_visible(&map.cursors["sessA"]));
    }

    #[test]
    fn a_cursor_left_of_the_main_screen_paints_nothing_there() {
        let mut map = empty_map();
        let state = placed(&mut map, "sessA");
        state.core.session_label = Some("A".to_owned());
        let painted = |map: &RenderMap| {
            let mut pm = tiny_skia::Pixmap::new(100, 100).unwrap();
            paint_main_screen(&mut pm, map, (0.0, 0.0), 1.0);
            pm.pixels().iter().any(|pixel| pixel.alpha() > 0)
        };
        assert!(painted(&map));
        assert!(cursor_may_paint(&map.cursors["sessA"]));

        // Placed on a display left of the main screen: the label would clamp
        // its badge into the main-screen pixmap if this cursor were painted.
        map.cursors.get_mut("sessA").unwrap().core.pos = (-200.0, 30.0);
        assert!(!painted(&map));
        assert!(cursor_may_paint(&map.cursors["sessA"]));
        assert!(cursor_is_externally_visible(&map.cursors["sessA"]));
    }

    /// Painting at the tile's origin rounds sub-pixel coordinates apart from
    /// the full-screen paint, which moves an antialiased edge by at most one
    /// of its 16 coverage samples.
    const COVERAGE_SAMPLE: u8 = 17;

    fn assert_tile_matches_full_screen(map: &RenderMap) {
        let scale = map.platform.backing_scale;
        let mut full = tiny_skia::Pixmap::new(
            (map.platform.win_w * scale) as u32,
            (map.platform.win_h * scale) as u32,
        )
        .unwrap();
        paint_main_screen(&mut full, map, (0.0, 0.0), scale as f32);
        let tile = paint_tile(map);
        let width = full.width() as usize;
        for (i, pixel) in full.pixels().iter().enumerate() {
            let (x, y) = (i % width, i / width);
            let in_tile = tile.as_ref().and_then(|tile| {
                let tx = x.checked_sub((tile.frame[0] * scale) as usize)?;
                let ty = y.checked_sub((tile.frame[1] * scale) as usize)?;
                (tx < tile.pixmap.width() as usize && ty < tile.pixmap.height() as usize)
                    .then(|| tile.pixmap.pixels()[ty * tile.pixmap.width() as usize + tx])
            });
            match in_tile {
                Some(tile_pixel) => {
                    let channels = |p: tiny_skia::PremultipliedColorU8| {
                        [p.red(), p.green(), p.blue(), p.alpha()]
                    };
                    let close = channels(tile_pixel)
                        .into_iter()
                        .zip(channels(*pixel))
                        .all(|(a, b)| a.abs_diff(b) <= COVERAGE_SAMPLE);
                    assert!(close, "pixel ({x}, {y}): {tile_pixel:?} vs {pixel:?}");
                }
                None => assert_eq!(pixel.alpha(), 0, "pixel ({x}, {y}) outside the tile"),
            }
        }
    }

    /// The tile holds every pixel the full-screen paint draws: cursor, badge,
    /// glide and click effects, also where the screen edge
    /// clamps the badge.
    #[test]
    fn the_tile_holds_every_pixel_of_the_full_screen_paint() {
        let starts = [(200.0, 150.0), (3.0, 4.0), (397.0, 296.0), (-30.0, 150.0)];
        let mut badge_frames = 0;
        let mut effect_frames = 0;
        for scale in [1.0, 2.0] {
            for (x, y) in starts {
                for label in [None, Some("a session with a long name")] {
                    {
                        let mut map = RenderMap::new(
                            CursorConfig::default(),
                            MacScreen {
                                win_w: 400.0,
                                win_h: 300.0,
                                backing_scale: scale,
                            },
                        );
                        let frame = Some(ScreenFrame::new(0.0, 0.0, 400.0, 300.0));
                        assert!(map.seed_start_if_unplaced("sessA", x, y, frame));
                        let cursor = map.cursors.get_mut("sessA").unwrap();
                        // Every motion effect is opt-in; turn them all on so the
                        // tile must hold them.
                        cursor.core.options_override = Some(cursor_overlay::CursorOptions::ALL_ON);
                        cursor.core.motion.effects = cursor_overlay::MotionEffects {
                            trail: Some(true),
                            glow: Some(true),
                            magnet: Some(true),
                            ripple: Some(true),
                            squish: Some(true),
                        };
                        if let Some(label) = label {
                            cursor.core.badge_mode_override =
                                Some(cursor_overlay::BadgeMode::Verbose);
                            cursor.apply_command(OverlayCommand::SetSessionLabel(label.to_owned()));
                        }
                        cursor.apply_command(OverlayCommand::MoveTo {
                            x: 400.0 - x,
                            y: 300.0 - y,
                            end_heading_radians: 0.0,
                            target: None,
                        });
                        for step in 0..40 {
                            if step == 20 {
                                let cursor = map.cursors.get_mut("sessA").unwrap();
                                let (x, y) = cursor.core.pos;
                                cursor.apply_command(OverlayCommand::ClickPulse { x, y });
                            }
                            map.tick_all(1.0 / 60.0);
                            if step % 4 == 1 {
                                badge_frames += usize::from(
                                    map.cursors["sessA"].core.session_badge_is_visible(),
                                );
                                effect_frames += usize::from(
                                    map.cursors["sessA"].core.effect_bounds().is_some(),
                                );
                                assert_tile_matches_full_screen(&map);
                            }
                        }
                    }
                }
            }
        }
        assert!(badge_frames > 0, "no frame showed the session badge");
        assert!(effect_frames > 0, "no frame played a motion effect");
    }

    #[test]
    fn a_custom_theme_paints_the_whole_screen() {
        let mut map = empty_map();
        placed(&mut map, "sessA");
        map.platform.win_w = 400.0;
        map.platform.win_h = 300.0;
        assert_ne!(paint_bounds(&map), Some([0.0, 0.0, 400.0, 300.0]));

        let mut theme = (*cursor_overlay::embedded_default_theme()).clone();
        theme.id = "com.example.custom".into();
        map.cursors.get_mut("sessA").unwrap().core.theme = Some(std::sync::Arc::new(theme));
        assert_eq!(paint_bounds(&map), Some([0.0, 0.0, 400.0, 300.0]));
    }

    #[test]
    fn removal_through_the_adapter_releases_only_that_arrival_waiter() {
        let mut map = empty_map();
        assert_eq!(
            apply_msg(
                &mut map,
                OverlayMsg::Cmd(KeyedOverlayCommand {
                    key: "sessA".to_owned(),
                    cmd: OverlayCommand::SetEnabled(true),
                }),
            )
            .as_deref(),
            Some("sessA")
        );
        assert_eq!(
            apply_msg(&mut map, OverlayMsg::Remove("sessA".to_owned())),
            None
        );
        assert!(!map.cursors.contains_key("sessA"));
    }

    #[test]
    fn resting_cursor_floats_until_idle_hide_and_never_hide_parks() {
        let mut map = empty_map();
        let cursor = placed(&mut map, "sessA");
        cursor.core.motion.idle_hide_ms = 20_000.0;
        // With `cursor.float` off the cursor rests still; the default floats.
        cursor.core.options_override = Some(cursor_overlay::CursorOptions::NONE);
        assert!(!cursor.core.has_resting_motion());
        cursor.core.options_override = Some(cursor_overlay::CursorOptions::default());
        assert!(cursor.core.has_resting_motion());
        assert!(map.needs_frame_tick());

        let cursor = map.cursors.get_mut("sessA").unwrap();
        cursor.core.motion.idle_hide_ms = 0.0;
        assert!(!map.needs_frame_tick(), "a never-hiding cursor rests still");
    }

    #[test]
    fn reduced_motion_countdown_keeps_macos_frames() {
        let mut map = empty_map();
        let cursor = placed(&mut map, "sessA");
        cursor.core.visual.reduced_motion = cursor_overlay::ReducedMotion::On;
        cursor.core.motion.idle_hide_ms = 20_000.0;
        // The macOS loop has no idle deadline, so the opaque countdown ticks.
        assert!(!cursor.core.needs_frame_tick());
        assert!(cursor.needs_frame_tick());

        cursor.core.motion.idle_hide_ms = 0.0;
        assert!(!cursor.needs_frame_tick());
        // A retired focus-rect command draws nothing and keeps no frames alive.
        cursor.core.options_override = Some(cursor_overlay::CursorOptions::NONE);
        cursor.apply_command(OverlayCommand::ShowFocusRect(Some([0.0, 0.0, 10.0, 10.0])));
        assert!(!cursor.needs_frame_tick());

        cursor.core.idle_alpha = 0.0;
        assert!(!map.needs_frame_tick(), "a fully hidden cursor must park");
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
