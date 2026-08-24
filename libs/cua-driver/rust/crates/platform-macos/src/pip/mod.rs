//! macOS multi-target Agent View.
//!
//! Floating NSWindow containing a bounded grid of exact native-window and
//! browser-tab cards, grouped by the existing lifecycle session.
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

use pip_preview::{PipBackend, PipBackendFactory, PipConfig, PipFrame, PipViewModel};

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
// Window, image view, and label pointers are stashed as `usize` so
// `Send` works (raw `*mut AnyObject` is `!Send`). The actual deref +
// `msg_send!` happens only on the main queue inside the dispatched
// block, so there is no thread-safety hazard from the Send promise.

struct NativeHandles {
    window: usize,
    content_view: usize,
}

static HANDLES: Mutex<Option<NativeHandles>> = Mutex::new(None);
static VIEW_MODEL: Mutex<Option<PipViewModel>> = Mutex::new(None);

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

    fn remove_workspace(&self, workspace_id: &str) {
        dispatch_to_main(workspace_id.to_owned(), remove_workspace_cb);
    }

    fn shutdown(self: Box<Self>) {
        dispatch_to_main((), shutdown_cb);
    }
}

unsafe extern "C" fn push_frame_cb(ctx: *mut c_void) {
    let frame: PipFrame = *Box::from_raw(ctx as *mut PipFrame);
    let frames = {
        let mut model = VIEW_MODEL.lock().unwrap();
        let model = model.get_or_insert_with(|| PipViewModel::new(12));
        model.upsert(frame);
        model
            .ordered_frames()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>()
    };
    render_frames(&frames);
}

unsafe extern "C" fn remove_workspace_cb(ctx: *mut c_void) {
    let workspace_id: String = *Box::from_raw(ctx as *mut String);
    let frames = {
        let mut model = VIEW_MODEL.lock().unwrap();
        let model = model.get_or_insert_with(|| PipViewModel::new(12));
        model.remove_workspace(&workspace_id);
        model
            .ordered_frames()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>()
    };
    render_frames(&frames);
}

unsafe fn ns_string(value: &str) -> *mut objc2::runtime::AnyObject {
    use objc2::{class, msg_send};

    let sanitized = value.replace('\0', " ");
    let Ok(cstr) = std::ffi::CString::new(sanitized) else {
        return std::ptr::null_mut();
    };
    msg_send![
        class!(NSString),
        stringWithUTF8String: cstr.as_ptr() as *const u8
    ]
}

unsafe fn add_text_label(
    parent: *mut objc2::runtime::AnyObject,
    frame: objc2_foundation::NSRect,
    text: &str,
    font_size: f64,
    bold: bool,
    secondary: bool,
) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    let label: *mut AnyObject = {
        let alloc: *mut AnyObject = msg_send![class!(NSTextField), alloc];
        msg_send![alloc, initWithFrame: frame]
    };
    let _: () = msg_send![label, setBezeled: false];
    let _: () = msg_send![label, setDrawsBackground: false];
    let _: () = msg_send![label, setEditable: false];
    let _: () = msg_send![label, setSelectable: false];
    let color: *mut AnyObject = if secondary {
        msg_send![
            class!(NSColor),
            colorWithCalibratedWhite: 0.78_f64
            alpha: 1.0_f64
        ]
    } else {
        msg_send![class!(NSColor), whiteColor]
    };
    let _: () = msg_send![label, setTextColor: color];
    let font: *mut AnyObject = if bold {
        msg_send![class!(NSFont), boldSystemFontOfSize: font_size]
    } else {
        msg_send![class!(NSFont), systemFontOfSize: font_size]
    };
    let _: () = msg_send![label, setFont: font];
    let value = ns_string(text);
    if !value.is_null() {
        let _: () = msg_send![label, setStringValue: value];
    }
    let _: () = msg_send![parent, addSubview: label];
}

unsafe fn render_frames(frames: &[PipFrame]) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let content_view = {
        let guard = HANDLES.lock().unwrap();
        match guard.as_ref() {
            Some(handles) => handles.content_view as *mut AnyObject,
            None => return,
        }
    };
    let empty: *mut AnyObject = msg_send![class!(NSArray), array];
    let _: () = msg_send![content_view, setSubviews: empty];

    let bounds: NSRect = msg_send![content_view, bounds];
    if frames.is_empty() {
        add_text_label(
            content_view,
            NSRect::new(
                NSPoint::new(16.0, (bounds.size.height - 24.0) / 2.0),
                NSSize::new((bounds.size.width - 32.0).max(40.0), 24.0),
            ),
            "Waiting for an exact window or browser tab...",
            12.0,
            false,
            true,
        );
        return;
    }

    let count = frames.len();
    let cols = match count {
        1 => 1,
        2..=4 => 2,
        _ => 3,
    };
    let rows = count.div_ceil(cols);
    let gap = 6.0_f64;
    let card_width = ((bounds.size.width - gap * (cols as f64 + 1.0)) / cols as f64).max(40.0);
    let card_height = ((bounds.size.height - gap * (rows as f64 + 1.0)) / rows as f64).max(40.0);
    let header_height = 20.0_f64.min(card_height * 0.22);
    let footer_height = 19.0_f64.min(card_height * 0.20);

    for (index, frame) in frames.iter().enumerate() {
        let col = index % cols;
        let row = index / cols;
        let x = gap + col as f64 * (card_width + gap);
        let y = bounds.size.height - gap - (row as f64 + 1.0) * card_height - row as f64 * gap;
        let card_rect = NSRect::new(NSPoint::new(x, y), NSSize::new(card_width, card_height));
        let card: *mut AnyObject = {
            let alloc: *mut AnyObject = msg_send![class!(NSView), alloc];
            msg_send![alloc, initWithFrame: card_rect]
        };
        let _: () = msg_send![card, setWantsLayer: true];
        let layer: *mut AnyObject = msg_send![card, layer];
        let _: () = msg_send![layer, setCornerRadius: 9.0_f64];
        let _: () = msg_send![layer, setMasksToBounds: true];
        let bg: *mut AnyObject = msg_send![
            class!(NSColor),
            colorWithCalibratedRed: 0.055_f64
            green: 0.065_f64
            blue: 0.075_f64
            alpha: 1.0_f64
        ];
        let bg_cg: *mut CGColor = msg_send![bg, CGColor];
        let _: () = msg_send![layer, setBackgroundColor: bg_cg];
        let accent: *mut AnyObject = match frame.target.target_kind {
            pip_preview::PipTargetKind::BrowserTab => msg_send![
                class!(NSColor),
                colorWithCalibratedRed: 0.15_f64
                green: 0.78_f64
                blue: 0.65_f64
                alpha: 0.95_f64
            ],
            pip_preview::PipTargetKind::NativeWindow => msg_send![
                class!(NSColor),
                colorWithCalibratedRed: 0.98_f64
                green: 0.52_f64
                blue: 0.20_f64
                alpha: 0.95_f64
            ],
        };
        let accent_cg: *mut CGColor = msg_send![accent, CGColor];
        let _: () = msg_send![layer, setBorderColor: accent_cg];
        let _: () = msg_send![layer, setBorderWidth: 1.5_f64];

        let image_height = (card_height - header_height - footer_height).max(1.0);
        let image_rect = NSRect::new(
            NSPoint::new(0.0, footer_height),
            NSSize::new(card_width, image_height),
        );
        let image_view: *mut AnyObject = {
            let alloc: *mut AnyObject = msg_send![class!(NSImageView), alloc];
            msg_send![alloc, initWithFrame: image_rect]
        };
        let _: () = msg_send![image_view, setImageScaling: 3u64];
        let ns_data: *mut AnyObject = msg_send![
            class!(NSData),
            dataWithBytes: frame.png_bytes.as_ptr() as *const c_void
            length: frame.png_bytes.len()
        ];
        if !ns_data.is_null() {
            let alloc: *mut AnyObject = msg_send![class!(NSImage), alloc];
            let image: *mut AnyObject = msg_send![alloc, initWithData: ns_data];
            if !image.is_null() {
                let _: () = msg_send![image_view, setImage: image];
            }
        }
        let _: () = msg_send![card, addSubview: image_view];

        add_text_label(
            card,
            NSRect::new(
                NSPoint::new(8.0, card_height - header_height),
                NSSize::new((card_width - 16.0).max(20.0), header_height),
            ),
            &frame.target.workspace_label,
            10.5,
            true,
            false,
        );
        let footer = format!("{} · {}", frame.target.target_label, frame.action_label);
        add_text_label(
            card,
            NSRect::new(
                NSPoint::new(8.0, 0.0),
                NSSize::new((card_width - 16.0).max(20.0), footer_height),
            ),
            &footer,
            9.5,
            false,
            true,
        );
        let _: () = msg_send![content_view, addSubview: card];
    }
}

unsafe extern "C" fn shutdown_cb(_ctx: *mut c_void) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    let handles = HANDLES.lock().unwrap().take();
    VIEW_MODEL.lock().unwrap().take();
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
/// serve --agent-view` so the dispatch_async_f → main queue
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
    // Borderless so the image owns the whole rectangle. No close button
    // / title bar — the window is owned by the daemon session lifecycle.
    // The rounded-corner look comes from a CALayer-backed content view
    // with cornerRadius + masksToBounds; the window itself stays
    // transparent outside the rounded rect.
    //   NSWindowStyleMaskBorderless = 0
    let style_mask: u64 = 0;
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

    // Transparent backing so the corners outside the CALayer-clipped
    // content view show whatever's underneath — gives the floating-pill
    // look. The shadow comes from AppKit's default `hasShadow: true`.
    let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
    let _: () = msg_send![win, setBackgroundColor: clear];
    let _: () = msg_send![win, setOpaque: false];
    let _: () = msg_send![win, setHasShadow: true];
    // Draggable from anywhere since there's no title bar.
    let _: () = msg_send![win, setMovableByWindowBackground: true];

    // Floating window level (NSFloatingWindowLevel = 3).
    let _: () = msg_send![win, setLevel: 3i64];

    // Collection behavior: visible across all spaces, no Mission
    // Control affordance, never the main / key window.
    // 1<<0 CanJoinAllSpaces | 1<<4 Stationary | 1<<8 FullScreenAuxiliary
    // 1<<6 Transient | 1<<7 IgnoresCycle
    let behavior: u64 = (1 << 0) | (1 << 4) | (1 << 8) | (1 << 6) | (1 << 7);
    let _: () = msg_send![win, setCollectionBehavior: behavior];

    let _: () = msg_send![win, setReleasedWhenClosed: false];
    let _: () = msg_send![win, setHidesOnDeactivate: false];

    // ── Content view: rounded-corner black backing ──
    // wantsLayer + masksToBounds clips the image view to the rounded
    // rect. The backing CALayer color shows wherever the (proportionally
    // scaled) image leaves gaps above/below or left/right.
    let content_view: *mut AnyObject = msg_send![win, contentView];
    let _: () = msg_send![content_view, setWantsLayer: true];
    let content_layer: *mut AnyObject = msg_send![content_view, layer];
    let _: () = msg_send![content_layer, setCornerRadius: 12.0_f64];
    let _: () = msg_send![content_layer, setMasksToBounds: true];
    let black: *mut AnyObject = msg_send![
        class!(NSColor),
        colorWithCalibratedRed: 0.0_f64
        green: 0.0_f64
        blue: 0.0_f64
        alpha: 1.0_f64
    ];
    let black_cg: *mut CGColor = msg_send![black, CGColor];
    let _: () = msg_send![content_layer, setBackgroundColor: black_cg];

    // Show the window without making it key or activating the app.
    let _: () = msg_send![win, orderFrontRegardless];

    *HANDLES.lock().unwrap() = Some(NativeHandles {
        window: win as usize,
        content_view: content_view as usize,
    });
    *VIEW_MODEL.lock().unwrap() = Some(PipViewModel::new(12));
    render_frames(&[]);

    tracing::info!(target: "pip", "Agent View initialised ({}x{})", cfg.geometry.width, cfg.geometry.height);
}
