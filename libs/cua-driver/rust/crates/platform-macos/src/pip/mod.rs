//! macOS picture-in-picture preview window.
//!
//! Floating NSWindow with an NSImageView showing a low-frame-rate live
//! preview of the main display. The preview window itself is excluded
//! from ScreenCaptureKit so it never produces a recursive mirror.
//!
//! ## Threading model
//!
//! Mirrors `cursor/overlay.rs`:
//!
//! - The MCP/tokio server runs on a background thread.
//! - AppKit MUST run on the main thread, which `cua-driver/src/main.rs`
//!   parks in `NSApplication.run()` for the cursor overlay.
//! - A ScreenCaptureKit stream runs on its own callback queue and sends
//!   retained `CGImage`s to the AppKit main queue via `dispatch_async_f`.
//! - At most one frame may be waiting for AppKit. Newer frames are dropped
//!   while that slot is occupied so a busy main thread cannot accumulate
//!   an unbounded queue.
//! - `push_frame()` remains as a post-action PNG fallback when live capture
//!   cannot start.
//!
//! ## Window properties
//!
//! - Standard titled, closable, miniaturizable, resizable window chrome.
//! - Default collection behavior, so the preview stays on the Space where the
//!   user opened it instead of following them across every desktop.
//! - `level = .floating` (kCGFloatingWindowLevel, between normal apps
//!   and dock; high enough to stay visible, low enough not to obscure
//!   menus or accessibility overlays).
//! - `setIgnoresMouseEvents(false)` and
//!   `setMovableByWindowBackground(true)` — the user can move, resize,
//!   minimize, or close the preview without affecting the daemon.
//! - No activation: `setHidesOnDeactivate(false)` and
//!   `setBecomesKeyOnlyIfNeeded(true)` so the window never steals
//!   keyboard focus from the user's frontmost app.
//!
//! ## Init lifecycle
//!
//! Because the cursor overlay already owns the main thread when
//! enabled, `MacosPipBackend::start` cannot block on it. Instead it
//! posts the window-creation block onto the main queue and returns
//! immediately. The first frame may arrive before the window exists;
//! that's fine — the push path reads the window pointer from a
//! `Mutex<Option<usize>>` and silently no-ops until init finishes.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pip_preview::{PipBackend, PipBackendFactory, PipConfig, PipFrame};
use screencapturekit::prelude::{
    CMSampleBufferExt, CMSampleBufferSCExt, CMTime, SCContentFilter, SCShareableContent, SCStream,
    SCStreamConfiguration, SCStreamOutputType,
};

// ── CGColor objc2 encoding shim ────────────────────────────────────────────
//
// `[NSColor CGColor]` returns a `CGColorRef` whose Objective-C type encoding
// is `^{CGColor=}`. objc2's strict msg_send! enforcement rejects bare
// `*mut c_void` (`^v`) for both sides of that call. Declare a phantom
// struct with the matching encoding so we can typed-cast through it
// without pulling in a wider CGColor binding crate.

#[repr(C)]
struct CGColor {
    _opaque: [u8; 0],
}

#[repr(C)]
struct NativeCGImage {
    _opaque: [u8; 0],
}

// RefEncode supplies an automatic Encode impl for `*mut CGColor` /
// `*const CGColor` via objc2's blanket — that's the route msg_send! needs
// for both setting layer.backgroundColor and reading [NSColor CGColor].
// `ENCODING_REF` is the encoding for one level of indirection, so the
// pointer wrap goes here (objc encoding `^{CGColor=}`).
unsafe impl objc2::RefEncode for CGColor {
    const ENCODING_REF: objc2::Encoding =
        objc2::Encoding::Pointer(&objc2::Encoding::Struct("CGColor", &[]));
}

unsafe impl objc2::RefEncode for NativeCGImage {
    const ENCODING_REF: objc2::Encoding =
        objc2::Encoding::Pointer(&objc2::Encoding::Struct("CGImage", &[]));
}

// ── Native AppKit pointer cell ─────────────────────────────────────────────
//
// Window and image-view pointers are stashed as `usize` so
// `Send` works (raw `*mut AnyObject` is `!Send`). The actual deref +
// `msg_send!` happens only on the main queue inside the dispatched
// block, so there is no thread-safety hazard from the Send promise.

struct NativeHandles {
    window: usize,
    image_view: usize,
}

static HANDLES: Mutex<Option<NativeHandles>> = Mutex::new(None);

const LIVE_CAPTURE_FPS: i32 = 8;
const LIVE_CAPTURE_MAX_SIDE: f64 = 1280.0;
const LIVE_CAPTURE_WINDOW_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

static LIVE_STREAM: Mutex<Option<SCStream>> = Mutex::new(None);
static LIVE_CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);
static LIVE_CAPTURE_CANCELLED: AtomicBool = AtomicBool::new(false);
static LIVE_FRAME_PENDING: AtomicBool = AtomicBool::new(false);

struct LiveFrame {
    image: screencapturekit::CGImage,
}

// ── libdispatch glue — same shape as cursor::overlay ──────────────────────

#[link(name = "dispatch", kind = "dylib")]
extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
}

fn dispatch_to_main<T: Send + 'static>(payload: T, cb: unsafe extern "C" fn(*mut c_void)) {
    let boxed = Box::new(payload);
    unsafe {
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(main_queue, Box::into_raw(boxed) as *mut c_void, cb);
    }
}

// ── Backend impl ──────────────────────────────────────────────────────────

pub struct MacosPipBackend;

impl PipBackend for MacosPipBackend {
    fn push_frame(&self, frame: PipFrame) {
        // A live ScreenCaptureKit stream is the primary presentation path on
        // macOS. Keep the existing post-action PNG bridge as a fallback for
        // machines where live capture could not be established.
        if LIVE_CAPTURE_ACTIVE.load(Ordering::Acquire) {
            return;
        }
        // No window yet? Drop the frame silently — start() dispatches
        // the create block onto the main queue and the very first
        // tool call can race that block.
        if HANDLES.lock().unwrap().is_none() {
            return;
        }
        dispatch_to_main(frame, push_frame_cb);
    }

    fn shutdown(self: Box<Self>) {
        stop_live_capture();
        dispatch_to_main((), shutdown_cb);
    }
}

fn live_capture_dimensions(width_points: u32, height_points: u32, scale: f64) -> (u32, u32) {
    let mut width = (f64::from(width_points) * scale.max(1.0)).max(1.0);
    let mut height = (f64::from(height_points) * scale.max(1.0)).max(1.0);
    let longest = width.max(height);
    if longest > LIVE_CAPTURE_MAX_SIDE {
        let shrink = LIVE_CAPTURE_MAX_SIDE / longest;
        width *= shrink;
        height *= shrink;
    }
    (width.round() as u32, height.round() as u32)
}

fn stop_live_capture() {
    LIVE_CAPTURE_CANCELLED.store(true, Ordering::Release);
    LIVE_CAPTURE_ACTIVE.store(false, Ordering::Release);
    LIVE_FRAME_PENDING.store(false, Ordering::Release);
    let stream = LIVE_STREAM
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(stream) = stream {
        if let Err(error) = stream.stop_capture() {
            tracing::debug!(target: "pip", %error, "failed to stop live PiP capture cleanly");
        }
    }
}

fn start_live_capture(window_id: u32, output_width: u32, output_height: u32) {
    LIVE_CAPTURE_CANCELLED.store(false, Ordering::Release);
    if let Err(error) = std::thread::Builder::new()
        .name("cua-pip-live-capture".into())
        .spawn(move || match build_live_capture(window_id, output_width, output_height) {
            Ok(stream) => {
                if LIVE_CAPTURE_CANCELLED.load(Ordering::Acquire) {
                    let _ = stream.stop_capture();
                    return;
                }
                *LIVE_STREAM
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(stream);
                LIVE_CAPTURE_ACTIVE.store(true, Ordering::Release);
                tracing::info!(
                    target: "pip",
                    fps = LIVE_CAPTURE_FPS,
                    width = output_width,
                    height = output_height,
                    "live PiP capture started"
                );
            }
            Err(error) => {
                tracing::warn!(target: "pip", %error, "live PiP capture unavailable; using post-action screenshots");
            }
        })
    {
        tracing::warn!(target: "pip", %error, "failed to spawn live PiP capture worker");
    }
}

fn build_live_capture(
    window_id: u32,
    output_width: u32,
    output_height: u32,
) -> anyhow::Result<SCStream> {
    let deadline = Instant::now() + LIVE_CAPTURE_WINDOW_LOOKUP_TIMEOUT;
    let (display, pip_window) = loop {
        let content = SCShareableContent::get()
            .map_err(|error| anyhow::anyhow!("SCShareableContent::get failed: {error}"))?;
        let main_display_id = unsafe { core_graphics::display::CGMainDisplayID() };
        let display = content
            .displays()
            .into_iter()
            .find(|display| display.display_id() == main_display_id)
            .or_else(|| content.displays().into_iter().next())
            .ok_or_else(|| anyhow::anyhow!("no displays available for live PiP capture"))?;
        if let Some(window) = content
            .windows()
            .into_iter()
            .find(|window| window.window_id() == window_id)
        {
            break (display, window);
        }
        if Instant::now() >= deadline {
            anyhow::bail!("PiP window {window_id} was not visible to ScreenCaptureKit");
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&[&pip_window])
        .build();
    let frame_interval = CMTime::new(1, LIVE_CAPTURE_FPS);
    let config = SCStreamConfiguration::new()
        .with_width(output_width)
        .with_height(output_height)
        .with_scales_to_fit(true)
        .with_preserves_aspect_ratio(true)
        .with_queue_depth(3)
        .with_minimum_frame_interval(&frame_interval)
        .with_shows_cursor(true);

    let mut stream = SCStream::new(&filter, &config);
    stream
        .add_output_handler(
            |sample: screencapturekit::cm::CMSampleBuffer, output_type: SCStreamOutputType| {
                if output_type != SCStreamOutputType::Screen
                    || LIVE_CAPTURE_CANCELLED.load(Ordering::Acquire)
                    || sample
                        .frame_status()
                        .is_some_and(|status| !status.has_content())
                    || LIVE_FRAME_PENDING.swap(true, Ordering::AcqRel)
                {
                    return;
                }

                match sample.cg_image() {
                    Ok(image) => dispatch_to_main(LiveFrame { image }, push_live_frame_cb),
                    Err(error) => {
                        LIVE_FRAME_PENDING.store(false, Ordering::Release);
                        tracing::debug!(target: "pip", error, "live PiP frame had no image");
                    }
                }
            },
            SCStreamOutputType::Screen,
        )
        .ok_or_else(|| anyhow::anyhow!("ScreenCaptureKit rejected the PiP output handler"))?;
    stream
        .start_capture()
        .map_err(|error| anyhow::anyhow!("SCStream::start_capture failed: {error}"))?;
    Ok(stream)
}

unsafe extern "C" fn push_live_frame_cb(ctx: *mut c_void) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSSize;

    let frame: LiveFrame = *Box::from_raw(ctx as *mut LiveFrame);
    LIVE_FRAME_PENDING.store(false, Ordering::Release);

    let (window_ptr, image_view_ptr) = {
        let guard = HANDLES.lock().unwrap();
        match guard.as_ref() {
            Some(handles) => (handles.window, handles.image_view),
            None => return,
        }
    };
    let window = window_ptr as *mut AnyObject;
    let minimized: bool = msg_send![window, isMiniaturized];
    if minimized {
        return;
    }
    let visible: bool = msg_send![window, isVisible];
    if !visible {
        stop_live_capture();
        return;
    }

    let cg_image = frame.image.as_ptr() as *mut NativeCGImage;
    let image: *mut AnyObject = {
        let alloc: *mut AnyObject = msg_send![class!(NSImage), alloc];
        msg_send![alloc, initWithCGImage: cg_image size: NSSize::new(0.0, 0.0)]
    };
    if !image.is_null() {
        let image_view = image_view_ptr as *mut AnyObject;
        let _: () = msg_send![image_view, setImage: image];
        let _: () = msg_send![image, release];
    }
}

unsafe extern "C" fn push_frame_cb(ctx: *mut c_void) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    let frame: PipFrame = *Box::from_raw(ctx as *mut PipFrame);

    let image_view_ptr = {
        let guard = HANDLES.lock().unwrap();
        match guard.as_ref() {
            Some(h) => h.image_view,
            None => return,
        }
    };

    // Construct NSData from the PNG bytes, then NSImage from NSData.
    // `dataWithBytes:length:` copies into a fresh NSData so the input
    // `Vec<u8>` can be freed at the end of this block.
    let png_ptr = frame.png_bytes.as_ptr() as *const c_void;
    let png_len = frame.png_bytes.len();
    let ns_data: *mut AnyObject = msg_send![
        class!(NSData),
        dataWithBytes: png_ptr
        length: png_len
    ];
    if ns_data.is_null() {
        return;
    }
    let img: *mut AnyObject = {
        let alloc: *mut AnyObject = msg_send![class!(NSImage), alloc];
        msg_send![alloc, initWithData: ns_data]
    };
    if !img.is_null() {
        let image_view = image_view_ptr as *mut AnyObject;
        let _: () = msg_send![image_view, setImage: img];
        let _: () = msg_send![img, release];
    }
}

unsafe extern "C" fn shutdown_cb(_ctx: *mut c_void) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    let handles = HANDLES.lock().unwrap().take();
    if let Some(h) = handles {
        let win = h.window as *mut AnyObject;
        if !win.is_null() {
            let _: () = msg_send![win, orderOut: std::ptr::null_mut::<AnyObject>()];
            let _: () = msg_send![win, close];
        }
    }
}

// ── AppKit main loop helper for Serve mode ───────────────────────────────

/// Park the main thread in `NSApplication.run()`. Used by `cua-driver
/// serve --experimental-pip` so the dispatch_async_f → main queue
/// path PiP frames go through can be drained. Mirrors the cursor
/// overlay's `run_appkit` startup (Accessory activation policy →
/// finishLaunching → run) without installing the overlay's
/// CALayer-backed window itself.
///
/// Never returns — the background `serve::run_serve_cmd` thread calls
/// `std::process::exit` when it finishes, which tears down NSApp at
/// the same time.
pub fn run_appkit_main_loop() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    let _mtm = objc2_foundation::MainThreadMarker::new()
        .expect("run_appkit_main_loop must be called from the main thread");
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        // Accessory policy: no Dock icon, no menu bar. Keeps the
        // daemon out of the user's application switcher, same as
        // the cursor overlay's NSApp setup.
        let _: bool = msg_send![app, setActivationPolicy: 1i64];
        let _: () = msg_send![app, finishLaunching];
        let _: () = msg_send![app, run];
    }
}

// ── Factory ──────────────────────────────────────────────────────────────

pub struct MacosPipBackendFactory;

impl PipBackendFactory for MacosPipBackendFactory {
    fn start(&self, cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>> {
        // Window construction must happen on the main thread. We hand
        // off via dispatch_async_f and return immediately — the first
        // few frames may be dropped while init races, which is fine
        // for a live-preview UX.
        let cfg_clone = cfg.clone();
        dispatch_to_main(cfg_clone, init_cb);
        Ok(Box::new(MacosPipBackend))
    }
}

unsafe extern "C" fn init_cb(ctx: *mut c_void) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let cfg: PipConfig = *Box::from_raw(ctx as *mut PipConfig);

    // Idempotency guard — `start()` should only be called once per
    // process, but cheap to defend against duplicate calls.
    if HANDLES.lock().unwrap().is_some() {
        return;
    }

    // ── Resolve geometry ──
    // AppKit windows use a bottom-left origin in screen coordinates.
    // The CLI flag uses a top-left X11-style origin (since that's the
    // mental model agents have for screenshots). Flip Y here so a
    // `+0+0` flag puts the window in the top-left corner.
    let screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
    if screen.is_null() {
        // Headless environment (CI) — skip silently. The daemon keeps
        // running without a PiP window.
        return;
    }
    let screen_frame: NSRect = msg_send![screen, frame];
    let backing_scale: f64 = msg_send![screen, backingScaleFactor];

    let w = cfg.geometry.width as f64;
    let h = cfg.geometry.height as f64;
    // Default placement: top-right corner with a 24pt inset, mirroring
    // the macOS conventions for floating utility windows.
    let inset = 24.0_f64;
    let (top_left_x, top_left_y) = match (cfg.geometry.x, cfg.geometry.y) {
        (Some(x), Some(y)) => (x as f64, y as f64),
        _ => (screen_frame.size.width - w - inset, inset),
    };
    // Convert top-left → bottom-left for AppKit.
    let bottom_y = screen_frame.size.height - top_left_y - h;
    let rect = NSRect::new(NSPoint::new(top_left_x, bottom_y), NSSize::new(w, h));

    // ── NSWindow ──
    // Use ordinary macOS window affordances. The preview is auxiliary, but it
    // must never trap the user behind an immovable borderless always-on-top
    // surface.
    //   Titled = 1<<0 | Closable = 1<<1 | Miniaturizable = 1<<2 |
    //   Resizable = 1<<3
    let style_mask: u64 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3);
    let backing_store_buffered: u64 = 2;
    let win: *mut AnyObject = {
        let alloc: *mut AnyObject = msg_send![class!(NSWindow), alloc];
        msg_send![
            alloc,
            initWithContentRect: rect
            styleMask: style_mask
            backing: backing_store_buffered
            defer: false
        ]
    };
    if win.is_null() {
        return;
    }

    if let Ok(cstr) = std::ffi::CString::new(cfg.title) {
        let title: *mut AnyObject = msg_send![
            class!(NSString),
            stringWithUTF8String: cstr.as_ptr() as *const u8
        ];
        if !title.is_null() {
            let _: () = msg_send![win, setTitle: title];
        }
    }
    let black: *mut AnyObject = msg_send![class!(NSColor), blackColor];
    let _: () = msg_send![win, setBackgroundColor: black];
    let _: () = msg_send![win, setOpaque: true];
    let _: () = msg_send![win, setHasShadow: true];
    let _: () = msg_send![win, setIgnoresMouseEvents: false];
    // In addition to the title bar, allow grabbing unused image background.
    let _: () = msg_send![win, setMovableByWindowBackground: true];

    // Floating window level (NSFloatingWindowLevel = 3).
    let _: () = msg_send![win, setLevel: 3i64];

    // Keep the default collection behavior. In particular, do not join every
    // Space or opt out of normal window cycling: those choices made the
    // preview feel permanently glued to the screen.
    let _: () = msg_send![win, setCollectionBehavior: 0u64];

    let _: () = msg_send![win, setReleasedWhenClosed: false];
    let _: () = msg_send![win, setHidesOnDeactivate: false];

    // ── Content view: black backing behind proportional screenshots ──
    let content_view: *mut AnyObject = msg_send![win, contentView];
    let _: () = msg_send![content_view, setWantsLayer: true];
    let content_layer: *mut AnyObject = msg_send![content_view, layer];
    let black_cg: *mut CGColor = msg_send![black, CGColor];
    let _: () = msg_send![content_layer, setBackgroundColor: black_cg];

    // ── NSImageView: fills the entire content view ──
    let image_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h));
    let image_view: *mut AnyObject = {
        let alloc: *mut AnyObject = msg_send![class!(NSImageView), alloc];
        msg_send![alloc, initWithFrame: image_rect]
    };
    // NSImageScaleProportionallyUpOrDown = 3 (preserve aspect ratio).
    // AppKit types this as NSUInteger — passing signed i64 triggers
    // an objc2 type-encoding panic on macOS 26+.
    let _: () = msg_send![image_view, setImageScaling: 3u64];
    // Follow user-initiated resize operations.
    let _: () = msg_send![image_view, setAutoresizingMask: 18u64];

    let _: () = msg_send![content_view, addSubview: image_view];

    // Show the window without making it key or activating the app.
    let _: () = msg_send![win, orderFrontRegardless];

    let window_number: i64 = msg_send![win, windowNumber];

    *HANDLES.lock().unwrap() = Some(NativeHandles {
        window: win as usize,
        image_view: image_view as usize,
    });

    if let Ok(window_id) = u32::try_from(window_number) {
        let (output_width, output_height) =
            live_capture_dimensions(cfg.geometry.width, cfg.geometry.height, backing_scale);
        start_live_capture(window_id, output_width, output_height);
    } else {
        tracing::warn!(target: "pip", window_number, "cannot start live PiP capture without a valid window id");
    }

    tracing::info!(target: "pip", "PiP window initialised ({}x{})", cfg.geometry.width, cfg.geometry.height);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_capture_dimensions_follow_backing_scale() {
        assert_eq!(live_capture_dimensions(320, 200, 2.0), (640, 400));
        assert_eq!(live_capture_dimensions(320, 200, 1.0), (320, 200));
    }

    #[test]
    fn live_capture_dimensions_bound_large_windows_without_changing_aspect_ratio() {
        assert_eq!(live_capture_dimensions(4000, 2000, 2.0), (1280, 640));
    }
}
