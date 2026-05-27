//! macOS picture-in-picture preview window.
//!
//! Floating NSWindow with an NSImageView showing the most recent
//! post-action screenshot and an NSTextField with a one-line label
//! describing the action that produced it.
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
//!   `[imageView setImage:]` + `[label setStringValue:]`.
//!
//! ## Window properties
//!
//! - `NSWindowCollectionBehaviorCanJoinAllSpaces | FullScreenAuxiliary |
//!    Stationary | Transient | IgnoresCycle`
//! - `level = .floating` (kCGFloatingWindowLevel, between normal apps
//!   and dock; high enough to stay visible, low enough not to obscure
//!   menus or accessibility overlays).
//! - `setIgnoresMouseEvents(false)` — user can click the red close
//!   button. Backend cleanup happens on `shutdown()`; closing the
//!   window manually decouples it from the session as the spec
//!   requires.
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

// ── Native AppKit pointer cell ─────────────────────────────────────────────
//
// Window, image view, and label pointers are stashed as `usize` so
// `Send` works (raw `*mut AnyObject` is `!Send`). The actual deref +
// `msg_send!` happens only on the main queue inside the dispatched
// block, so there is no thread-safety hazard from the Send promise.

struct NativeHandles {
    window: usize,
    image_view: usize,
    label: usize,
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

    let (image_view_ptr, label_ptr) = {
        let guard = HANDLES.lock().unwrap();
        match guard.as_ref() {
            Some(h) => (h.image_view, h.label),
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

    // Update the label. NSString::stringWithUTF8String requires NUL
    // termination; copy into a CString so we hand a clean buffer.
    if let Ok(cstr) = std::ffi::CString::new(frame.action_label) {
        let ns_str: *mut AnyObject = msg_send![
            class!(NSString),
            stringWithUTF8String: cstr.as_ptr() as *const u8
        ];
        if !ns_str.is_null() {
            let label = label_ptr as *mut AnyObject;
            let _: () = msg_send![label, setStringValue: ns_str];
        }
    }
}

unsafe extern "C" fn shutdown_cb(_ctx: *mut c_void) {
    use objc2::runtime::AnyObject;
    use objc2::msg_send;

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
    // Titled + Closable so the user can dismiss; no Resizable /
    // Miniaturizable to keep the chrome minimal.
    //   NSWindowStyleMaskTitled    = 1 << 0
    //   NSWindowStyleMaskClosable  = 1 << 1
    //   NSWindowStyleMaskNonactivatingPanel is panel-only — we use
    //   NSWindow + a no-activate collection behavior instead, which
    //   achieves the same "never steals focus" outcome without
    //   forcing the caller to instantiate an NSPanel.
    let style_mask: u64 = (1 << 0) | (1 << 1);
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

    // Title — set via stringWithUTF8String.
    if let Ok(cstr) = std::ffi::CString::new(cfg.title.clone()) {
        let ns_str: *mut AnyObject = msg_send![
            class!(NSString),
            stringWithUTF8String: cstr.as_ptr() as *const u8
        ];
        if !ns_str.is_null() {
            let _: () = msg_send![win, setTitle: ns_str];
        }
    }

    // Floating window level (kCGFloatingWindowLevel = 3 above NSNormal).
    // NSFloatingWindowLevel = 3.
    let _: () = msg_send![win, setLevel: 3i64];

    // Collection behavior: visible across all spaces, no Mission
    // Control affordance, never the main / key window.
    // 1<<0 CanJoinAllSpaces | 1<<4 Stationary | 1<<8 FullScreenAuxiliary
    // 1<<6 Transient | 1<<7 IgnoresCycle
    let behavior: u64 = (1 << 0) | (1 << 4) | (1 << 8) | (1 << 6) | (1 << 7);
    let _: () = msg_send![win, setCollectionBehavior: behavior];

    let _: () = msg_send![win, setReleasedWhenClosed: false];
    let _: () = msg_send![win, setHidesOnDeactivate: false];
    // Note: `becomesKeyOnlyIfNeeded` is an NSPanel-only selector and
    // sending it to NSWindow crashes with NSInvalidArgumentException.
    // We use `orderFrontRegardless` (below) instead of `makeKeyAndOrderFront`,
    // which already avoids stealing key-window status from the user's
    // frontmost app. Combined with the Transient / IgnoresCycle
    // collection-behavior flags above, this is enough to keep the PiP
    // window passive even when the user clicks inside it.

    // ── Content view layout ──
    // Image fills the top of the content view, leaving a 32pt strip
    // at the bottom for the action label.
    let label_height = 28.0_f64;
    let image_rect = NSRect::new(
        NSPoint::new(0.0, label_height),
        NSSize::new(w, h - label_height),
    );
    let label_rect = NSRect::new(
        NSPoint::new(8.0, 4.0),
        NSSize::new(w - 16.0, label_height - 8.0),
    );

    // NSImageView
    let image_view: *mut AnyObject = {
        let alloc: *mut AnyObject = msg_send![class!(NSImageView), alloc];
        msg_send![alloc, initWithFrame: image_rect]
    };
    // NSImageScaleProportionallyUpOrDown = 3 (preserve aspect ratio,
    // scale up or down to fit the view). AppKit types this as NSUInteger
    // (`'Q'` / u64) — a signed i64 here triggers an objc2 type-encoding
    // panic on macOS 26+.
    let _: () = msg_send![image_view, setImageScaling: 3u64];

    // NSTextField label
    let label: *mut AnyObject = {
        let alloc: *mut AnyObject = msg_send![class!(NSTextField), alloc];
        msg_send![alloc, initWithFrame: label_rect]
    };
    let _: () = msg_send![label, setBezeled: false];
    let _: () = msg_send![label, setDrawsBackground: false];
    let _: () = msg_send![label, setEditable: false];
    let _: () = msg_send![label, setSelectable: false];
    // Initial placeholder text — overwritten on the first frame.
    if let Ok(cstr) = std::ffi::CString::new("waiting for first action…") {
        let ns_str: *mut AnyObject = msg_send![
            class!(NSString),
            stringWithUTF8String: cstr.as_ptr() as *const u8
        ];
        if !ns_str.is_null() {
            let _: () = msg_send![label, setStringValue: ns_str];
        }
    }

    let content_view: *mut AnyObject = msg_send![win, contentView];
    let _: () = msg_send![content_view, addSubview: image_view];
    let _: () = msg_send![content_view, addSubview: label];

    // Show the window without making it key or activating the app —
    // `orderFrontRegardless` brings it on screen without disturbing
    // the active app's focus state.
    let _: () = msg_send![win, orderFrontRegardless];

    *HANDLES.lock().unwrap() = Some(NativeHandles {
        window: win as usize,
        image_view: image_view as usize,
        label: label as usize,
    });

    tracing::info!(target: "pip", "PiP window initialised ({}x{})", cfg.geometry.width, cfg.geometry.height);
}
