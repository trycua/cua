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
//! NSWindow covers `NSScreen.mainScreen.frame` which AppKit places with
//! a bottom-left origin, so we flip Y when drawing into the Pixmap.
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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cursor_overlay::{
    CursorConfig, CursorKey, FocusRect, KeyedOverlayCommand, MotionConfig, MsgOutcome,
    OverlayCommand, OverlayMsg, RenderEntry, RenderStateCore, ScreenFrame, ZOrderEnforcer,
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
static OVERLAY_WINDOW_ID: AtomicU32 = AtomicU32::new(0);

pub(crate) fn is_overlay_window(window_id: u32) -> bool {
    window_id != 0 && OVERLAY_WINDOW_ID.load(Ordering::Acquire) == window_id
}

/// CGWindowID of the live overlay window, if one is on screen.
pub(crate) fn overlay_window_id() -> Option<u32> {
    match OVERLAY_WINDOW_ID.load(Ordering::Acquire) {
        0 => None,
        window_id => Some(window_id),
    }
}

/// Whether the last composed frame could hold visible pixels, and when that
/// was last true. The layer update reaches WindowServer asynchronously, so a
/// frame that just cleared is still treated as visible for a short grace.
static OVERLAY_FRAME_PAINTS: AtomicBool = AtomicBool::new(false);
static OVERLAY_LAST_PAINT: Mutex<Option<Instant>> = Mutex::new(None);
const OVERLAY_CLEAR_GRACE: Duration = Duration::from_millis(250);

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

fn cursor_may_paint(state: &RenderState) -> bool {
    state.focus_rect.is_some()
        || (state.core.cfg.enabled
            && state.core.visible
            && state.core.idle_alpha > 0.0
            && state.core.pos.0 > -50.0
            && state.core.pos.1 > -50.0)
}

/// Screen-global geometry kept beside the shared keyed render map
/// ([`cursor_overlay::RenderMap`], which owns the per-session lifecycle:
/// lazy creation, stable z-order, tombstones, revival, and the default guard).
/// Written once in `run_appkit`.
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
                backing_scale: 1.0, // overwritten in run_appkit() once the NSScreen is known
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
            };
            send_command(session, cmd);
        },
    ));
}

/// Send a keyed command from any thread (MCP tool, etc.).  Non-blocking; drops
/// if the channel is full (old commands are less important than new ones).
pub fn send_command(key: CursorKey, cmd: OverlayCommand) {
    // Empty key is the explicit no-cursor sentinel for direct platform calls
    // that bypass lifecycle dispatch.
    if key.is_empty() {
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
/// No-op when the cursor is already on-screen (pos.0 > -50.0) or absent. The
/// seed is clamped to the main screen frame so it never starts off-display.
/// Returns true if a seed was applied (i.e. the cursor was at the sentinel and
/// is now primed to glide).
fn seed_start_if_sentinel(key: &CursorKey, target_x: f64, target_y: f64) -> bool {
    let mut guard = RENDER.lock().unwrap();
    let Some(map) = guard.as_mut() else {
        return false;
    };
    // `win_w/h` are 0 until the AppKit window is up; the shared seed then only
    // keeps the start point off negative coordinates.
    let MacScreen { win_w, win_h, .. } = map.platform;
    let frame = (win_w > 0.0 && win_h > 0.0).then(|| ScreenFrame::new(0.0, 0.0, win_w, win_h));
    map.seed_start_if_sentinel(key, target_x, target_y, frame)
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
        matches!(
            guard.as_ref().and_then(|m| m.cursors.get(&key)),
            Some(rs) if rs.core.cfg.enabled && rs.core.pos.0 > -50.0
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
        },
    );

    // Await arrival signal (fired from render thread when Dubins path ends).
    let _ = rx.await;
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

impl RenderEntry for RenderState {
    fn from_config(cfg: CursorConfig) -> Self {
        RenderState {
            core: RenderStateCore::new(cfg),
            focus_rect: None,
            focus_rect_t: 1.0,
        }
    }

    fn core(&self) -> &RenderStateCore {
        &self.core
    }

    fn core_mut(&mut self) -> &mut RenderStateCore {
        &mut self.core
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

    fn apply_command(&mut self, cmd: OverlayCommand) -> bool {
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
                true
            }
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
    /// idle fade) plus two macOS terms: the focus-rect fade, and the opaque
    /// idle-hide countdown. The macOS loop parks on a blocking `recv` with no
    /// deadline, so a reduced-motion cursor must keep frame ticks through the
    /// countdown for its idle fade to start on time.
    fn needs_frame_tick(&self) -> bool {
        self.core.needs_frame_tick()
            || self.focus_rect.is_some()
            || (self.core.motion.idle_hide_ms > 0.0
                && self.core.visible
                && self.core.pos.0 >= -100.0
                && self.core.idle_alpha >= 0.004)
    }
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
    // Finish launching without presenting a UI (needed for NSApp.run())
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
    // Fall back to the CG estimator (current-mode pixels ÷ points) when
    // the AppKit call returns a non-positive value, since downstream paint
    // math divides by this and a 0.0 would zero out the cursor.
    let mut backing_scale: f64 = msg_send![main_screen, backingScaleFactor];
    if backing_scale.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        use core_graphics::display::CGMainDisplayID;
        let display_id = CGMainDisplayID();
        backing_scale = crate::tools::get_screen_size::get_backing_scale(display_id);
        if backing_scale.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            backing_scale = 1.0;
        }
    }

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
        return;
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

    // Set layer geometry. contentsScale tells Core Animation that the CGImage
    // we hand to setContents: is already at retina (`backing_scale`×) pixel
    // density — without this, CA would treat our physical-pixel pixmap as a
    // 1× asset and bilinear-downsample it back to logical pixels on screen,
    // re-introducing the blur this pipeline exists to eliminate.
    let _: () = msg_send![layer, setContentsScale: backing_scale];
    // kCAGravityTopLeft — the string literal "topLeft"
    let gravity_ns: *mut AnyObject = msg_send![class!(NSString),
        stringWithUTF8String: c"topLeft".as_ptr().cast::<u8>()
    ];
    let _: () = msg_send![layer, setContentsGravity: gravity_ns];

    // ---- Update RenderMap header with screen size (screen-global) ----
    {
        let mut guard = RENDER.lock().unwrap();
        if let Some(m) = guard.as_mut() {
            m.platform = MacScreen {
                win_w,
                win_h,
                backing_scale,
            };
        }
    }

    let window_number: isize = msg_send![win, windowNumber];
    OVERLAY_WINDOW_ID.store(u32::try_from(window_number).unwrap_or(0), Ordering::Release);

    // ---- Show the window ----
    let _: () = msg_send![win, orderFrontRegardless];

    // ---- Render thread (60 fps) ----
    let layer_ptr = layer as usize;
    let win_ptr = win as usize;
    std::thread::spawn(move || {
        render_loop(layer_ptr, win_ptr, rx, win_w, win_h);
    });

    // ---- NSApplication run loop (blocks until process exits) ----
    let _: () = msg_send![app, run];
    OVERLAY_WINDOW_ID.store(0, Ordering::Release);
}

fn render_loop(
    layer_ptr: usize,
    win_ptr: usize,
    rx: std::sync::mpsc::Receiver<OverlayMsg>,
    _win_w: f64,
    _win_h: f64,
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
            match rx.recv() {
                Ok(msg) => (Some(msg), false),
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

        // ── Phase 1: drain + tick all cursors (one lock acquisition) ──────
        // `pinned_wid` follows the most-recently-updated cursor: a single
        // NSWindow can occupy only one z-band, so the last-active cursor's
        // target wins. `arrived` collects the keys whose path just ended.
        let (
            pinned_wid,
            raise_unpinned,
            arrived,
            win_w,
            win_h,
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
                        map.platform.win_w,
                        map.platform.win_h,
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
            if pinned_wid.is_some() && (pin_changed || repin_frames >= 60) {
                MacZOrderEnforcer { win_ptr }.reassert(pinned_wid);
                repin_frames = 0;
            } else if raise_unpinned {
                // A direct move_cursor has no target window to pin against.
                // Raise the normal-level, click-through overlay without
                // activating the driver so a later foreground application
                // cannot cover a standalone session cursor.
                dispatch_order_front(win_ptr);
                repin_frames = 0;
            } else if repin_frames >= 60 {
                repin_frames = 0;
            }
        }

        // ── Phase 2: composite every cursor into ONE pixmap ───────────────
        // Render only when a command arrived or the previous/next tick can
        // change pixels. A final frame is emitted as animations/fades finish so
        // the layer is left in the completed/cleared state before blocking.
        if had_msg || hover_changed || frame_tick_needed || next_frame_tick_needed {
            let pixmap = {
                let guard = RENDER.lock().unwrap();
                if let Some(map) = guard.as_ref() {
                    // Allocate the pixmap at the screen's PHYSICAL pixel
                    // dimensions so the cursor rasterises at retina resolution.
                    // The cursor's logical coordinates are scaled into pixmap
                    // pixels inside `paint_cursor` (it multiplies px/py/sizes
                    // by `backing_scale`).
                    let scale = map.platform.backing_scale.max(1.0);
                    let w = (win_w * scale).max(1.0) as u32;
                    let h = (win_h * scale).max(1.0) as u32;
                    let mut pm = tiny_skia::Pixmap::new(w.max(1), h.max(1))
                        .unwrap_or_else(|| tiny_skia::Pixmap::new(1, 1).unwrap());
                    let backing_scale_f32 = scale as f32;
                    note_overlay_frame(map.cursors.values().any(cursor_may_paint));
                    for (_k, rs) in &map.cursors {
                        let focus = rs.focus_rect.map(|rect| FocusRect {
                            rect,
                            t: rs.focus_rect_t,
                        });
                        cursor_overlay::paint_cursor(
                            &mut pm,
                            &rs.core,
                            0.0,
                            0.0, // macOS uses screen-local coords (no origin offset)
                            focus,
                            backing_scale_f32,
                        );
                    }
                    pm
                } else {
                    break;
                }
            };

            // Convert to CGImage and update layer on the main queue.
            dispatch_set_layer_contents(layer_ptr, pixmap);
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
        && state.core.pos.0 > -50.0
        && state.core.pos.1 > -50.0
        && state.core.idle_alpha >= 0.004
}

/// Convert a `tiny_skia::Pixmap` to a `CGImage` and set it as the contents
/// of the given `CALayer` via `dispatch_async(main_queue, ...)`.
fn dispatch_set_layer_contents(layer_ptr: usize, pixmap: tiny_skia::Pixmap) {
    // Build the CGImage from the pixmap bytes.
    let cg_image_ptr = match pixmap_to_cgimage(&pixmap) {
        Some(p) => p,
        None => return,
    };

    // Box the payload for the C callback.
    let payload = Box::new((layer_ptr, cg_image_ptr));

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
        let (layer_ptr, cg_image_ptr): (usize, usize) = *Box::from_raw(ctx as *mut _);
        let layer = layer_ptr as *mut objc2::runtime::AnyObject;
        // setContents: expects an `id` (type '@'), not a raw void pointer.
        // CGImageRef is toll-free bridged to NSObject, so we cast it to *mut AnyObject.
        let cg_id = cg_image_ptr as *mut objc2::runtime::AnyObject;
        let _: () = objc2::msg_send![layer, setContents: cg_id];
        // Release the CGImage ref we retained in pixmap_to_cgimage.
        CGImageRelease(cg_image_ptr as *mut c_void);
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
    use std::collections::HashMap;

    // The keyed lifecycle, sentinel seed, and shared frame-tick predicate are
    // covered once in `cursor_overlay::render_map`. These tests cover only the
    // macOS adapter: window ordering, external visibility, and the macOS
    // frame-tick terms layered on the shared predicate.

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
        assert!(map.seed_start_if_sentinel(key, 60.0, 60.0, frame));
        map.cursors.get_mut(key).unwrap()
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

        map.cursors.get_mut("sessA").unwrap().core.cfg.enabled = false;
        assert!(!cursor_is_externally_visible(&map.cursors["sessA"]));
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
        assert!(cursor.core.has_resting_motion());
        assert!(map.needs_frame_tick());

        let cursor = map.cursors.get_mut("sessA").unwrap();
        cursor.core.motion.idle_hide_ms = 0.0;
        assert!(!map.needs_frame_tick(), "a never-hiding cursor rests still");
    }

    #[test]
    fn focus_rect_and_reduced_motion_countdown_keep_macos_frames() {
        let mut map = empty_map();
        let cursor = placed(&mut map, "sessA");
        cursor.core.visual.reduced_motion = cursor_overlay::ReducedMotion::On;
        cursor.core.motion.idle_hide_ms = 20_000.0;
        // The macOS loop has no idle deadline, so the opaque countdown ticks.
        assert!(!cursor.core.needs_frame_tick());
        assert!(cursor.needs_frame_tick());

        cursor.core.motion.idle_hide_ms = 0.0;
        assert!(!cursor.needs_frame_tick());
        cursor.apply_command(OverlayCommand::ShowFocusRect(Some([0.0, 0.0, 10.0, 10.0])));
        assert!(cursor.needs_frame_tick());
        for _ in 0..60 {
            cursor.tick(1.0 / 60.0);
        }
        assert!(cursor.focus_rect.is_none());

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
