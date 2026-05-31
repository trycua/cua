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

// ── Global overlay state ──────────────────────────────────────────────────

static CMD_TX: OnceLock<std::sync::mpsc::SyncSender<OverlayMsg>> = OnceLock::new();
// Single-consumer slot; receiver is moved into run_on_main_thread().
static CMD_RX_CELL: Mutex<Option<std::sync::mpsc::Receiver<OverlayMsg>>> = Mutex::new(None);
static RENDER: Mutex<Option<RenderMap>> = Mutex::new(None);

/// The keyed, insertion-ordered collection of owned cursors that the render
/// loop composites every frame. Insertion order = stable z-order (later keys
/// paint on top). `win_w` / `win_h` are screen-global, hoisted out of the
/// per-cursor `RenderState` (written once in `run_appkit`).
struct RenderMap {
    cursors: IndexMap<CursorKey, RenderState>,
    win_w: f64,
    win_h: f64,
    /// Frozen launch-time config used as the template for lazily-created
    /// cursors (its palette is overridden per-key via `Palette::for_instance`).
    template: CursorConfig,
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
                if let Ok(mut guard) = ARRIVAL_TX.lock() {
                    if let Some(m) = guard.as_mut() {
                        m.remove(&key);
                    }
                }
            }
            None
        }
        OverlayMsg::Cmd(KeyedOverlayCommand { key, cmd }) => {
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

/// Initialise global overlay state (call once, before run_on_main_thread).
pub fn init(cfg: CursorConfig) {
    let (tx, rx) = std::sync::mpsc::sync_channel(4096);
    let _ = CMD_TX.set(tx);
    *CMD_RX_CELL.lock().unwrap() = Some(rx);
    *ARRIVAL_TX.lock().unwrap() = Some(HashMap::new());
    let mut cursors = IndexMap::new();
    cursors.insert("default".to_owned(), RenderState::new(cfg.clone()));
    *RENDER.lock().unwrap() = Some(RenderMap {
        cursors,
        win_w: 0.0,
        win_h: 0.0,
        template: cfg,
    });
}

/// Send a keyed command from any thread (MCP tool, etc.).  Non-blocking; drops
/// if the channel is full (old commands are less important than new ones).
pub fn send_command(key: CursorKey, cmd: OverlayCommand) {
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(OverlayMsg::Cmd(KeyedOverlayCommand { key, cmd }));
    }
}

/// Convenience for callsites not yet threaded with a session key: drives the
/// seeded `"default"` cursor (the anonymous / one-shot identity).
pub fn send_command_default(cmd: OverlayCommand) {
    send_command("default".to_owned(), cmd);
}

/// Remove a session's owned cursor from the render collection (fired from the
/// `session_end` hook). The `"default"` key is guarded against removal on the
/// render side, so this is a no-op for it; removing an absent key (anonymous
/// session that never created a cursor) is a harmless no-op.
pub fn remove_cursor(key: CursorKey) {
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(OverlayMsg::Remove(key));
    }
}

/// Return a snapshot of the current motion config (for use by set_agent_cursor_motion
/// to apply partial overrides without losing other knobs). Reads the
/// `"default"` cursor's motion (motion is a shared timing config in practice).
pub fn current_motion() -> MotionConfig {
    RENDER.lock().unwrap()
        .as_ref()
        .and_then(|m| m.cursors.get("default").map(|rs| rs.core.motion.clone()))
        .unwrap_or_default()
}

/// Animate the overlay cursor to `(x, y)` and suspend until the Dubins path
/// completes and the spring overshoot begins.
///
/// Mirrors Swift's `AgentCursor.shared.animateAndWait(to:)`.
/// Returns immediately (no animation) when:
/// - the overlay is disabled, or
/// - the cursor is still at the off-screen sentinel `(-200, -200)` — in that
///   case the caller should rely on `ClickPulse` to snap the cursor.
pub async fn animate_cursor_to(key: CursorKey, x: f64, y: f64) {
    // Check whether animation should run for THIS cursor. A not-yet-created
    // cursor (sentinel position) relies on ClickPulse to snap, same as before.
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
    arrival_register(key.clone(), tx);

    // Send the MoveTo command (click offset applied inside apply_command).
    send_command(key, OverlayCommand::MoveTo {
        x,
        y,
        // Arrive pointing upper-left (45°), matching the macOS system-cursor
        // convention and Swift reference (`endAngleDegrees: 45`).
        end_heading_radians: std::f64::consts::FRAC_PI_4,
    });

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
            loop { std::thread::park(); }
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
        loop { std::thread::park(); }
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
    if win.is_null() { return; }

    let _: () = msg_send![win, setOpaque: false];
    let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
    let _: () = msg_send![win, setBackgroundColor: clear];
    let _: () = msg_send![win, setHasShadow: false];
    let _: () = msg_send![win, setIgnoresMouseEvents: true];
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

    // Set layer geometry
    let _: () = msg_send![layer, setContentsScale: 1.0_f64];
    // kCAGravityTopLeft — the string literal "topLeft"
    let gravity_ns: *mut AnyObject = msg_send![class!(NSString),
        stringWithUTF8String: b"topLeft\0".as_ptr()
    ];
    let _: () = msg_send![layer, setContentsGravity: gravity_ns];

    // ---- Update RenderMap header with screen size (screen-global) ----
    {
        let mut guard = RENDER.lock().unwrap();
        if let Some(m) = guard.as_mut() {
            m.win_w = win_w;
            m.win_h = win_h;
        }
    }

    // ---- Show the window ----
    let _: () = msg_send![win, orderFrontRegardless];

    // ---- Render thread (60 fps) ----
    let layer_ptr = layer as usize;
    let win_ptr   = win as usize;
    std::thread::spawn(move || {
        render_loop(layer_ptr, win_ptr, rx, win_w, win_h);
    });

    // ---- NSApplication run loop (blocks until process exits) ----
    let _: () = msg_send![app, run];
}

fn render_loop(
    layer_ptr: usize,
    win_ptr:   usize,
    rx: std::sync::mpsc::Receiver<OverlayMsg>,
    _win_w: f64, _win_h: f64,
) {
    let target_frame_ms = Duration::from_millis(16); // ~60 fps
    let mut last_tick = Instant::now();
    // Repin bookkeeping: track last pinned wid and a frame counter for
    // the periodic defensive-repin (every ~60 frames ≈ 1 s).
    let mut last_pinned: Option<u64> = None;
    let mut repin_frames: u32 = 0;

    loop {
        let now = Instant::now();
        let dt = now.duration_since(last_tick).as_secs_f64().min(0.05);
        last_tick = now;

        // ── Phase 1: drain + tick all cursors (one lock acquisition) ──────
        // `pinned_wid` follows the most-recently-updated cursor: a single
        // NSWindow can occupy only one z-band, so the last-active cursor's
        // target wins. `arrived` collects the keys whose path just ended.
        let (pinned_wid, arrived, win_w, win_h) = {
            let mut guard = RENDER.lock().unwrap();
            match guard.as_mut() {
                Some(map) => {
                    // Drain via get-or-create; track the last-touched key so we
                    // can read its pinned_wid after ticking.
                    let mut last_key: Option<CursorKey> = None;
                    while let Ok(msg) = rx.try_recv() {
                        if let Some(k) = apply_msg(map, msg) {
                            last_key = Some(k);
                        }
                    }
                    // Tick every cursor; record the ones that just arrived.
                    let mut arrived: Vec<CursorKey> = Vec::new();
                    for (k, rs) in map.cursors.iter_mut() {
                        if rs.tick(dt) {
                            arrived.push(k.clone());
                        }
                    }
                    let pinned = last_key
                        .as_ref()
                        .and_then(|k| map.cursors.get(k))
                        .map(|rs| rs.core.pinned_wid)
                        .unwrap_or(None);
                    (pinned, arrived, map.win_w, map.win_h)
                }
                None => break,
            }
        };

        // Fire arrival signals so each session's animate_cursor_to() unblocks.
        for k in &arrived {
            arrival_fire(k);
        }

        // Repin: immediately on target change, then defensive every ~1 s.
        // Delegate to the cross-platform ZOrderEnforcer so the contract for
        // "z+1 of the application under test" is documented once in
        // `cursor_overlay::z_order`.
        repin_frames += 1;
        let pin_changed = pinned_wid != last_pinned;
        last_pinned = pinned_wid;
        if pinned_wid.is_some() && (pin_changed || repin_frames >= 60) {
            MacZOrderEnforcer { win_ptr }.reassert(pinned_wid);
            repin_frames = 0;
        } else if repin_frames >= 60 {
            repin_frames = 0;
        }

        // ── Phase 2: composite every cursor into ONE pixmap ───────────────
        // Allocate the full-screen pixmap once per frame, then paint each
        // owned cursor into it (alpha-over, insertion order = z-order). The
        // expensive CGImage copy + setContents happen once regardless of N;
        // idle/hidden cursors early-return inside paint_cursor.
        let pixmap = {
            let guard = RENDER.lock().unwrap();
            if let Some(map) = guard.as_ref() {
                let w = win_w.max(1.0) as u32;
                let h = win_h.max(1.0) as u32;
                let mut pm = tiny_skia::Pixmap::new(w.max(1), h.max(1))
                    .unwrap_or_else(|| tiny_skia::Pixmap::new(1, 1).unwrap());
                for (_k, rs) in &map.cursors {
                    let focus = rs.focus_rect.map(|rect| FocusRect {
                        rect,
                        t: rs.focus_rect_t,
                    });
                    cursor_overlay::paint_cursor(
                        &mut pm,
                        &rs.core,
                        0.0, 0.0, // macOS uses screen-local coords (no origin offset)
                        focus,
                    );
                }
                pm
            } else {
                break;
            }
        };

        // Convert to CGImage and update layer on the main queue.
        dispatch_set_layer_contents(layer_ptr, pixmap);

        // Sleep remainder of frame budget.
        let elapsed = Instant::now().duration_since(last_tick);
        if let Some(remaining) = target_frame_ms.checked_sub(elapsed) {
            std::thread::sleep(remaining);
        }
    }
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
    #[link(name = "dispatch", kind = "dylib")]
    extern "C" {
        // Opaque placeholder — we only ever take &_dispatch_main_q, never read it.
        static _dispatch_main_q: u8;
        fn dispatch_async_f(queue: *const c_void, context: *mut c_void,
                            work: unsafe extern "C" fn(*mut c_void));
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

    extern "C" { fn CGImageRelease(image: *mut c_void); }

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
/// `target = None` is treated as a no-op here rather than raising to the
/// front: macOS creates the overlay at `NSNormalWindowLevel` and never
/// promotes it into an "always-on-top" level the way Windows does with
/// `HWND_TOPMOST`, so there is no topmost band to escape. Letting the
/// overlay stay where the user's activations have left it keeps it out
/// of the way when no agent action is in flight.
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
    if w == 0 || h == 0 { return None; }

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
                width: usize, height: usize,
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
            w, h,
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

        if img.is_null() { None } else { Some(img as usize) }
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
        cursors.insert("default".to_owned(), RenderState::new(CursorConfig::default()));
        RenderMap {
            cursors,
            win_w: 100.0,
            win_h: 100.0,
            template: CursorConfig::default(),
        }
    }

    fn move_msg(key: &str, x: f64, y: f64) -> OverlayMsg {
        OverlayMsg::Cmd(KeyedOverlayCommand {
            key: key.to_owned(),
            cmd: OverlayCommand::MoveTo { x, y, end_heading_radians: 0.0 },
        })
    }

    #[test]
    fn two_sessions_produce_two_distinct_render_entries() {
        let mut map = empty_map();
        apply_msg(&mut map, OverlayMsg::Cmd(KeyedOverlayCommand {
            key: "sessA".to_owned(),
            cmd: OverlayCommand::SetEnabled(true),
        }));
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
        assert!(matches!(rxb.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
    }
}
