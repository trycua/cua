//! macOS picture-in-picture preview window.
//!
//! Floating NSWindow with an NSImageView showing the most recent
//! post-action screenshot.
//!
//! ## Threading model
//!
//! Mirrors `cursor/overlay.rs`:
//!
//! - The MCP/tokio server runs on a background thread.
//! - AppKit MUST run on the main thread, which `cua-driver/src/main.rs`
//!   parks in `NSApplication.run()` for the cursor overlay.
//! - `push_frame()` is called from arbitrary tokio tasks. It packages
//!   the frame into a heap-allocated `Box` and posts the actual UI
//!   update onto the main queue via `dispatch_async_f`. The block
//!   then constructs an `NSImage` from the PNG bytes and calls
//!   `[imageView setImage:]`.
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
use std::sync::Mutex;

use pip_preview::{PipBackend, PipBackendFactory, PipConfig, PipFrame};

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

// RefEncode supplies an automatic Encode impl for `*mut CGColor` /
// `*const CGColor` via objc2's blanket — that's the route msg_send! needs
// for both setting layer.backgroundColor and reading [NSColor CGColor].
// `ENCODING_REF` is the encoding for one level of indirection, so the
// pointer wrap goes here (objc encoding `^{CGColor=}`).
unsafe impl objc2::RefEncode for CGColor {
    const ENCODING_REF: objc2::Encoding =
        objc2::Encoding::Pointer(&objc2::Encoding::Struct("CGColor", &[]));
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
        // No window yet? Drop the frame silently — start() dispatches
        // the create block onto the main queue and the very first
        // tool call can race that block.
        if HANDLES.lock().unwrap().is_none() {
            return;
        }
        dispatch_to_main(frame, push_frame_cb);
    }

    fn shutdown(self: Box<Self>) {
        dispatch_to_main((), shutdown_cb);
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

    *HANDLES.lock().unwrap() = Some(NativeHandles {
        window: win as usize,
        image_view: image_view as usize,
    });

    tracing::info!(target: "pip", "PiP window initialised ({}x{})", cfg.geometry.width, cfg.geometry.height);
}
