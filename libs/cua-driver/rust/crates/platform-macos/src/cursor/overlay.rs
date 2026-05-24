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

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cursor_overlay::{
    CursorConfig, FocusRect, MotionConfig, OverlayCommand, RenderStateCore,
};

// ── Arrival-signal channel ────────────────────────────────────────────────

static ARRIVAL_TX: Mutex<Option<tokio::sync::oneshot::Sender<()>>> = Mutex::new(None);

// ── Global overlay state ──────────────────────────────────────────────────

static CMD_TX: OnceLock<std::sync::mpsc::SyncSender<OverlayCommand>> = OnceLock::new();
// Single-consumer slot; receiver is moved into run_on_main_thread().
static CMD_RX_CELL: Mutex<Option<std::sync::mpsc::Receiver<OverlayCommand>>> = Mutex::new(None);
static RENDER: Mutex<Option<RenderState>> = Mutex::new(None);

/// Initialise global overlay state (call once, before run_on_main_thread).
pub fn init(cfg: CursorConfig) {
    let (tx, rx) = std::sync::mpsc::sync_channel(4096);
    let _ = CMD_TX.set(tx);
    *CMD_RX_CELL.lock().unwrap() = Some(rx);
    *RENDER.lock().unwrap() = Some(RenderState::new(cfg));
}

/// Send a command from any thread (MCP tool, etc.).  Non-blocking; drops if
/// the channel is full (old commands are less important than new ones).
pub fn send_command(cmd: OverlayCommand) {
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(cmd);
    }
}

/// Return a snapshot of the current motion config (for use by set_agent_cursor_motion
/// to apply partial overrides without losing other knobs).
pub fn current_motion() -> MotionConfig {
    RENDER.lock().unwrap()
        .as_ref()
        .map(|rs| rs.core.motion.clone())
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
pub async fn animate_cursor_to(x: f64, y: f64) {
    // Check whether animation should run.
    let should_animate = {
        let guard = RENDER.lock().unwrap();
        match guard.as_ref() {
            Some(rs) if rs.core.cfg.enabled && rs.core.pos.0 > -50.0 => true,
            _ => false,
        }
    };
    if !should_animate {
        return;
    }

    // Create a one-shot channel; store the sender so the render thread can
    // fire it when the path finishes.
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    {
        let mut guard = ARRIVAL_TX.lock().unwrap();
        // Cancel any previous waiter (superseded by new animation).
        if let Some(old_tx) = guard.take() {
            let _ = old_tx.send(());
        }
        *guard = Some(tx);
    }

    // Send the MoveTo command (click offset applied inside apply_command).
    send_command(OverlayCommand::MoveTo {
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
        match &*guard {
            Some(rs) => rs.core.cfg.clone(),
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
    /// Window (frame) size in NSScreen points (top-left origin after Y-flip).
    win_w: f64,
    win_h: f64,
    /// Focus-highlight rectangle `[x, y, w, h]` in screen coords; None = not shown.
    focus_rect: Option<[f64; 4]>,
    /// Fade progress for the focus rect: 0.0 = fully visible, 1.0 = gone.
    focus_rect_t: f64,
}

impl RenderState {
    fn new(cfg: CursorConfig) -> Self {
        RenderState {
            core: RenderStateCore::new(cfg),
            win_w: 0.0,
            win_h: 0.0,
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

unsafe fn run_appkit(_cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayCommand>) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

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

    // ---- Update RenderState with screen size ----
    {
        let mut guard = RENDER.lock().unwrap();
        if let Some(rs) = guard.as_mut() {
            rs.win_w = win_w;
            rs.win_h = win_h;
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
    rx: std::sync::mpsc::Receiver<OverlayCommand>,
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

        // Drain incoming commands and tick animation.
        let (pinned_wid, fire_arrival) = {
            let mut guard = RENDER.lock().unwrap();
            match guard.as_mut() {
                Some(rs) => {
                    while let Ok(cmd) = rx.try_recv() {
                        rs.apply_command(cmd);
                    }
                    let arrived = rs.tick(dt);
                    (rs.core.pinned_wid, arrived)
                }
                None => break,
            }
        };

        // Fire arrival signal so animate_cursor_to() can unblock.
        if fire_arrival {
            if let Ok(mut guard) = ARRIVAL_TX.lock() {
                if let Some(tx) = guard.take() {
                    let _ = tx.send(());
                }
            }
        }

        // Repin: immediately on target change, then defensive every ~1 s.
        repin_frames += 1;
        let pin_changed = pinned_wid != last_pinned;
        last_pinned = pinned_wid;
        if let Some(wid) = pinned_wid {
            if pin_changed || repin_frames >= 60 {
                dispatch_pin_above(win_ptr, wid);
                repin_frames = 0;
            }
        } else if repin_frames >= 60 {
            repin_frames = 0;
        }

        // Render frame.
        let pixmap = {
            let guard = RENDER.lock().unwrap();
            if let Some(rs) = guard.as_ref() {
                let focus = rs.focus_rect.map(|rect| FocusRect {
                    rect,
                    t: rs.focus_rect_t,
                });
                cursor_overlay::render_frame(
                    &rs.core,
                    rs.win_w.max(1.0) as u32,
                    rs.win_h.max(1.0) as u32,
                    0.0, 0.0, // macOS uses screen-local coords (no origin offset)
                    focus,
                )
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
