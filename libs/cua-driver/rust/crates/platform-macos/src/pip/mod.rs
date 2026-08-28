//! macOS multi-target Agent View.
//!
//! A floating, resizable miniature desktop that uses the host wallpaper and
//! presents exact native windows and browser tabs as aspect-aware macOS-like
//! windows. Existing lifecycle sessions group presentation only; Agent View
//! never claims, moves, resizes, or closes the underlying targets.

use std::ffi::{c_void, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use pip_preview::{
    layout_desktop, layout_session_tabs, png_dimensions, LayoutRect, PipBackend, PipBackendFactory,
    PipConfig, PipFrame, PipTargetKind, PipViewModel, PipWorkspaceSummary, SessionTabsLayout,
    TargetSize,
};

#[repr(C)]
struct CGColor {
    _opaque: [u8; 0],
}

unsafe impl objc2::RefEncode for CGColor {
    const ENCODING_REF: objc2::Encoding =
        objc2::Encoding::Pointer(&objc2::Encoding::Struct("CGColor", &[]));
}

struct NativeHandles {
    window: usize,
    canvas_view: usize,
    shell_wallpaper_container: usize,
    shell_wallpaper_view: usize,
    wallpaper_container: usize,
    wallpaper_view: usize,
    delegate: usize,
}

/// Outer corner radius of the Agent View container chrome.
const CONTAINER_RADIUS: f64 = 15.0;
const PIP_CORNER_RADIUS: f64 = 12.0;
/// The miniature desktop sits inside an asymmetric hardware-like glass shell.
const SHELL_SIDE_INSET: f64 = 9.0;
const SHELL_TOP_INSET: f64 = 20.0;
const SHELL_BOTTOM_INSET: f64 = 10.0;
const SESSION_SELECTOR_HEIGHT: f64 = 34.0;
const RESIZE_HIT_INSET: f64 = 7.0;

// Keep the PiP shell visually clean for now; invisible hit zones still provide
// dragging and resizing without exposing a native or custom frame.
const SHOW_SHELL_BORDER: bool = false;

const RESIZE_LEFT: isize = 1;
const RESIZE_RIGHT: isize = 2;
const RESIZE_BOTTOM: isize = 4;
const RESIZE_TOP: isize = 8;

static HANDLES: Mutex<Option<NativeHandles>> = Mutex::new(None);
static VIEW_MODEL: Mutex<Option<PipViewModel>> = Mutex::new(None);

#[link(name = "dispatch", kind = "dylib")]
extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
    fn dispatch_sync_f(
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

fn dispatch_to_main_sync<T>(payload: T, cb: unsafe extern "C" fn(*mut c_void)) {
    let context = Box::into_raw(Box::new(payload)) as *mut c_void;
    unsafe {
        if libc::pthread_main_np() != 0 {
            cb(context);
        } else {
            let main_queue = &raw const _dispatch_main_q as *const c_void;
            dispatch_sync_f(main_queue, context, cb);
        }
    }
}

pub struct MacosPipBackend;

struct InputPassthroughRequest {
    passthrough: bool,
    applied: Arc<AtomicBool>,
}

impl PipBackend for MacosPipBackend {
    fn push_frame(&self, frame: PipFrame) {
        // init_cb was queued first on the serial main queue, so retaining the
        // frame here avoids dropping exact activity during daemon startup.
        dispatch_to_main(frame, push_frame_cb);
    }

    fn remove_workspace(&self, workspace_id: &str) {
        dispatch_to_main(workspace_id.to_owned(), remove_workspace_cb);
    }

    fn remove_target(&self, workspace_id: &str, identity_key: &str) {
        dispatch_to_main(
            (workspace_id.to_owned(), identity_key.to_owned()),
            remove_target_cb,
        );
    }

    fn set_input_passthrough(&self, passthrough: bool) -> anyhow::Result<()> {
        let applied = Arc::new(AtomicBool::new(false));
        dispatch_to_main_sync(
            InputPassthroughRequest {
                passthrough,
                applied: Arc::clone(&applied),
            },
            set_input_passthrough_cb,
        );
        // The user may close the optional presentation window while the
        // daemon continues serving tools; that must never break input.
        let _ = applied.load(Ordering::Acquire);
        Ok(())
    }

    fn shutdown(self: Box<Self>) {
        dispatch_to_main((), shutdown_cb);
    }
}

unsafe extern "C" fn set_input_passthrough_cb(ctx: *mut c_void) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    let request: InputPassthroughRequest = *Box::from_raw(ctx as *mut InputPassthroughRequest);
    let handles = HANDLES.lock().unwrap();
    if let Some(handles) = handles.as_ref() {
        let window = handles.window as *mut AnyObject;
        let _: () = msg_send![window, setIgnoresMouseEvents: request.passthrough];
        request.applied.store(true, Ordering::Release);
    }
}

unsafe extern "C" fn push_frame_cb(ctx: *mut c_void) {
    let frame: PipFrame = *Box::from_raw(ctx as *mut PipFrame);
    let snapshot = {
        let mut model = VIEW_MODEL.lock().unwrap();
        let model = model.get_or_insert_with(|| PipViewModel::new(12));
        model.upsert(frame);
        clone_snapshot(model)
    };
    render_snapshot(&snapshot);
}

unsafe extern "C" fn remove_workspace_cb(ctx: *mut c_void) {
    let workspace_id: String = *Box::from_raw(ctx as *mut String);
    let snapshot = {
        let mut model = VIEW_MODEL.lock().unwrap();
        let model = model.get_or_insert_with(|| PipViewModel::new(12));
        model.remove_workspace(&workspace_id);
        clone_snapshot(model)
    };
    render_snapshot(&snapshot);
}

unsafe extern "C" fn remove_target_cb(ctx: *mut c_void) {
    let (workspace_id, identity_key): (String, String) =
        *Box::from_raw(ctx as *mut (String, String));
    let snapshot = {
        let mut model = VIEW_MODEL.lock().unwrap();
        let model = model.get_or_insert_with(|| PipViewModel::new(12));
        model.remove_target(&workspace_id, &identity_key);
        clone_snapshot(model)
    };
    render_snapshot(&snapshot);
}

struct ViewSnapshot {
    frames: Vec<PipFrame>,
    workspaces: Vec<PipWorkspaceSummary>,
    selected_workspace_id: Option<String>,
    active_view_id: Option<String>,
}

fn clone_snapshot(model: &PipViewModel) -> ViewSnapshot {
    ViewSnapshot {
        frames: model.selected_frames().into_iter().cloned().collect(),
        workspaces: model.workspaces(),
        selected_workspace_id: model.selected_workspace_id().map(str::to_owned),
        active_view_id: model.active_view_id().map(str::to_owned),
    }
}

fn current_snapshot() -> ViewSnapshot {
    VIEW_MODEL
        .lock()
        .unwrap()
        .as_ref()
        .map(clone_snapshot)
        .unwrap_or_else(|| ViewSnapshot {
            frames: Vec::new(),
            workspaces: Vec::new(),
            selected_workspace_id: None,
            active_view_id: None,
        })
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

unsafe fn rust_string(value: *mut objc2::runtime::AnyObject) -> Option<String> {
    use objc2::msg_send;

    if value.is_null() {
        return None;
    }
    let utf8: *const std::os::raw::c_char = msg_send![value, UTF8String];
    (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
}

#[derive(Clone, Copy)]
enum LabelTone {
    Muted,
    Dark,
}

unsafe fn color(red: f64, green: f64, blue: f64, alpha: f64) -> *mut objc2::runtime::AnyObject {
    use objc2::{class, msg_send};

    msg_send![
        class!(NSColor),
        colorWithCalibratedRed: red
        green: green
        blue: blue
        alpha: alpha
    ]
}

unsafe fn set_layer_background(
    layer: *mut objc2::runtime::AnyObject,
    background: *mut objc2::runtime::AnyObject,
) {
    use objc2::msg_send;

    let cg_color: *mut CGColor = msg_send![background, CGColor];
    let _: () = msg_send![layer, setBackgroundColor: cg_color];
}

/// Opt a container layer into the macOS squircle corner curve so the chrome
/// reads as continuous rather than as a circular-arc rectangle.
unsafe fn set_continuous_corners(layer: *mut objc2::runtime::AnyObject) {
    use objc2::msg_send;

    let responds: bool = msg_send![layer, respondsToSelector: objc2::sel!(setCornerCurve:)];
    if !responds {
        return;
    }
    let curve = ns_string("continuous");
    if curve.is_null() {
        return;
    }
    let _: () = msg_send![layer, setCornerCurve: curve];
}

unsafe fn set_layer_border(
    layer: *mut objc2::runtime::AnyObject,
    width: f64,
    border: *mut objc2::runtime::AnyObject,
) {
    use objc2::msg_send;

    let cg_color: *mut CGColor = msg_send![border, CGColor];
    let _: () = msg_send![layer, setBorderWidth: width];
    let _: () = msg_send![layer, setBorderColor: cg_color];
}

unsafe fn add_text_label(
    parent: *mut objc2::runtime::AnyObject,
    frame: objc2_foundation::NSRect,
    text: &str,
    font_size: f64,
    bold: bool,
    tone: LabelTone,
    alignment: isize,
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
    let _: () = msg_send![label, setAlignment: alignment];
    let text_color = match tone {
        LabelTone::Muted => color(1.0, 1.0, 1.0, 0.66),
        LabelTone::Dark => color(0.12, 0.13, 0.15, 0.94),
    };
    let _: () = msg_send![label, setTextColor: text_color];
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

unsafe fn rounded_view(
    frame: objc2_foundation::NSRect,
    radius: f64,
    background: *mut objc2::runtime::AnyObject,
    clips: bool,
) -> *mut objc2::runtime::AnyObject {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    let view: *mut AnyObject = {
        let alloc: *mut AnyObject = msg_send![class!(NSView), alloc];
        msg_send![alloc, initWithFrame: frame]
    };
    let _: () = msg_send![view, setWantsLayer: true];
    let layer: *mut AnyObject = msg_send![view, layer];
    let _: () = msg_send![layer, setCornerRadius: radius];
    let _: () = msg_send![layer, setMasksToBounds: clips];
    set_layer_background(layer, background);
    view
}

/// A layer-hosting view whose backing layer is a vertical `CAGradientLayer`.
///
/// The chrome body needs a top-to-bottom falloff to read as a lit glass
/// surface rather than as a flat outline. AppKit keeps a view-assigned layer
/// sized to its view, so this survives live resize without a manual pass.
/// Layer-hosting views must not take subviews; overlay strips go on the
/// enclosing chrome view instead.
unsafe fn gradient_view(
    frame: objc2_foundation::NSRect,
    radius: f64,
    top: *mut objc2::runtime::AnyObject,
    bottom: *mut objc2::runtime::AnyObject,
) -> *mut objc2::runtime::AnyObject {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    let view: *mut AnyObject = {
        let alloc: *mut AnyObject = msg_send![class!(NSView), alloc];
        msg_send![alloc, initWithFrame: frame]
    };
    let layer: *mut AnyObject = {
        let alloc: *mut AnyObject = msg_send![class!(CAGradientLayer), alloc];
        msg_send![alloc, init]
    };
    let top_cg: *mut CGColor = msg_send![top, CGColor];
    let bottom_cg: *mut CGColor = msg_send![bottom, CGColor];
    let stops: [*mut CGColor; 2] = [top_cg, bottom_cg];
    let colors: *mut AnyObject = msg_send![
        class!(NSArray),
        arrayWithObjects: stops.as_ptr() as *const *mut AnyObject
        count: 2usize
    ];
    let _: () = msg_send![layer, setColors: colors];
    let _: () = msg_send![layer, setStartPoint: objc2_foundation::NSPoint::new(0.5, 1.0)];
    let _: () = msg_send![layer, setEndPoint: objc2_foundation::NSPoint::new(0.5, 0.0)];
    let _: () = msg_send![layer, setCornerRadius: radius];
    let _: () = msg_send![layer, setMasksToBounds: true];
    set_continuous_corners(layer);
    let _: () = msg_send![view, setLayer: layer];
    let _: () = msg_send![view, setWantsLayer: true];
    view
}

unsafe fn visual_effect_view(
    frame: objc2_foundation::NSRect,
    radius: f64,
    material: isize,
    blending_mode: isize,
    clips: bool,
) -> *mut objc2::runtime::AnyObject {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    let view: *mut AnyObject = {
        let alloc: *mut AnyObject = msg_send![class!(NSVisualEffectView), alloc];
        msg_send![alloc, initWithFrame: frame]
    };
    let _: () = msg_send![view, setMaterial: material];
    let _: () = msg_send![view, setBlendingMode: blending_mode];
    let _: () = msg_send![view, setState: 1isize];
    let _: () = msg_send![view, setWantsLayer: true];
    let layer: *mut AnyObject = msg_send![view, layer];
    let _: () = msg_send![layer, setCornerRadius: radius];
    let _: () = msg_send![layer, setMasksToBounds: clips];
    view
}

unsafe fn add_circle(
    parent: *mut objc2::runtime::AnyObject,
    x: f64,
    y: f64,
    diameter: f64,
    fill: *mut objc2::runtime::AnyObject,
) {
    use objc2::msg_send;
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let circle = rounded_view(
        NSRect::new(NSPoint::new(x, y), NSSize::new(diameter, diameter)),
        diameter / 2.0,
        fill,
        true,
    );
    let _: () = msg_send![parent, addSubview: circle];
}

fn appkit_rect(rect: pip_preview::LayoutRect, bounds_height: f64) -> objc2_foundation::NSRect {
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    NSRect::new(
        NSPoint::new(rect.x, bounds_height - rect.y - rect.height),
        NSSize::new(rect.width, rect.height),
    )
}

fn parse_native_pid(target_id: &str) -> Option<i32> {
    let mut parts = target_id.split(':');
    (parts.next()? == "window")
        .then(|| parts.next()?.parse::<i32>().ok())
        .flatten()
}

fn truncate_label(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

unsafe fn target_identity(frame: &PipFrame) -> (String, *mut objc2::runtime::AnyObject, char) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    if frame.target.target_kind == PipTargetKind::NativeWindow {
        if let Some(pid) = parse_native_pid(&frame.target.target_id) {
            let app: *mut AnyObject = msg_send![
                class!(NSRunningApplication),
                runningApplicationWithProcessIdentifier: pid
            ];
            if !app.is_null() {
                let name: *mut AnyObject = msg_send![app, localizedName];
                let icon: *mut AnyObject = msg_send![app, icon];
                if let Some(name) = rust_string(name) {
                    let fallback = name.chars().next().unwrap_or('A').to_ascii_uppercase();
                    return (truncate_label(&name, 28), icon, fallback);
                }
            }
        }
    }

    let label = match frame.target.target_kind {
        PipTargetKind::BrowserTab => {
            if frame.target.target_label.trim().is_empty() {
                "Browser".to_owned()
            } else {
                truncate_label(&frame.target.target_label, 28)
            }
        }
        PipTargetKind::NativeWindow => truncate_label(&frame.target.workspace_label, 28),
    };
    let fallback = match frame.target.target_kind {
        PipTargetKind::BrowserTab => 'B',
        PipTargetKind::NativeWindow => label.chars().next().unwrap_or('A').to_ascii_uppercase(),
    };
    (label, std::ptr::null_mut(), fallback)
}

unsafe fn image_from_png(bytes: &[u8]) -> *mut objc2::runtime::AnyObject {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    let data: *mut AnyObject = msg_send![
        class!(NSData),
        dataWithBytes: bytes.as_ptr() as *const c_void
        length: bytes.len()
    ];
    if data.is_null() {
        return std::ptr::null_mut();
    }
    let alloc: *mut AnyObject = msg_send![class!(NSImage), alloc];
    msg_send![alloc, initWithData: data]
}

unsafe fn render_target_window(
    canvas: *mut objc2::runtime::AnyObject,
    frame: &PipFrame,
    layout: pip_preview::TargetLayout,
    bounds_height: f64,
    active: bool,
) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let window_frame = appkit_rect(layout.window, bounds_height);
    let radius = (window_frame.size.width.min(window_frame.size.height) * 0.035).clamp(5.0, 9.0);
    let shadow = rounded_view(window_frame, radius, color(0.0, 0.0, 0.0, 0.0), false);
    let shadow_layer: *mut AnyObject = msg_send![shadow, layer];
    let _: () = msg_send![shadow_layer, setShadowOpacity: 0.30_f32];
    let _: () = msg_send![shadow_layer, setShadowRadius: 16.0_f64];
    let _: () = msg_send![shadow_layer, setShadowOffset: NSSize::new(0.0, -6.0)];
    let black = color(0.0, 0.0, 0.0, 0.90);
    let black_cg: *mut CGColor = msg_send![black, CGColor];
    let _: () = msg_send![shadow_layer, setShadowColor: black_cg];

    let window = rounded_view(
        NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(window_frame.size.width, window_frame.size.height),
        ),
        radius,
        color(0.0, 0.0, 0.0, 0.0),
        true,
    );
    let window_layer: *mut AnyObject = msg_send![window, layer];
    if active {
        set_layer_border(window_layer, 2.2, color(0.25, 0.65, 1.0, 0.96));
    } else if SHOW_SHELL_BORDER {
        set_layer_border(window_layer, 0.55, color(0.0, 0.0, 0.0, 0.22));
    }
    let image_view: *mut AnyObject = {
        let alloc: *mut AnyObject = msg_send![class!(NSImageView), alloc];
        msg_send![
            alloc,
            initWithFrame: NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(window_frame.size.width, window_frame.size.height),
            )
        ]
    };
    let _: () = msg_send![image_view, setImageScaling: 3u64];
    let image = image_from_png(&frame.png_bytes);
    if !image.is_null() {
        let _: () = msg_send![image_view, setImage: image];
    }
    let _: () = msg_send![window, addSubview: image_view];
    if let Some((normalized_x, normalized_y)) = frame.cursor_position {
        let pointer_size = NSSize::new(18.0, 22.0);
        let x = (normalized_x.clamp(0.0, 1.0) * window_frame.size.width)
            .clamp(0.0, (window_frame.size.width - pointer_size.width).max(0.0));
        let y_from_top = normalized_y.clamp(0.0, 1.0) * window_frame.size.height;
        let y = (window_frame.size.height - y_from_top - pointer_size.height).clamp(
            0.0,
            (window_frame.size.height - pointer_size.height).max(0.0),
        );
        let pointer: *mut AnyObject = {
            let alloc: *mut AnyObject = msg_send![class!(NSImageView), alloc];
            msg_send![
                alloc,
                initWithFrame: NSRect::new(NSPoint::new(x, y), pointer_size)
            ]
        };
        let arrow_cursor: *mut AnyObject = msg_send![class!(NSCursor), arrowCursor];
        let arrow_image: *mut AnyObject = msg_send![arrow_cursor, image];
        let _: () = msg_send![pointer, setImage: arrow_image];
        let _: () = msg_send![pointer, setImageScaling: 0u64];
        let _: () = msg_send![pointer, setWantsLayer: true];
        let pointer_layer: *mut AnyObject = msg_send![pointer, layer];
        let _: () = msg_send![pointer_layer, setShadowOpacity: 0.72_f32];
        let _: () = msg_send![pointer_layer, setShadowRadius: 2.5_f64];
        let _: () = msg_send![pointer_layer, setShadowOffset: NSSize::new(0.0, -1.0)];
        let shadow_color = color(0.0, 0.0, 0.0, 0.92);
        let shadow_cg: *mut CGColor = msg_send![shadow_color, CGColor];
        let _: () = msg_send![pointer_layer, setShadowColor: shadow_cg];
        let _: () = msg_send![window, addSubview: pointer];
    }
    let _: () = msg_send![shadow, addSubview: window];
    let _: () = msg_send![canvas, addSubview: shadow];
}

unsafe fn render_dock(
    canvas: *mut objc2::runtime::AnyObject,
    frames: &[PipFrame],
    layout: &pip_preview::DesktopLayout,
    bounds_height: f64,
) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let dock_frame = appkit_rect(layout.dock, bounds_height);
    let dock = visual_effect_view(dock_frame, dock_frame.size.height * 0.30, 6, 1, true);
    let appearance_name = ns_string("NSAppearanceNameVibrantDark");
    let dark_appearance: *mut AnyObject =
        msg_send![class!(NSAppearance), appearanceNamed: appearance_name];
    if !dark_appearance.is_null() {
        let _: () = msg_send![dock, setAppearance: dark_appearance];
    }
    let dock_layer: *mut AnyObject = msg_send![dock, layer];
    set_layer_background(dock_layer, color(0.30, 0.32, 0.34, 0.22));
    set_layer_border(dock_layer, 0.65, color(1.0, 1.0, 1.0, 0.20));
    let _: () = msg_send![dock_layer, setShadowOpacity: 0.34_f32];
    let _: () = msg_send![dock_layer, setShadowRadius: 14.0_f64];
    let _: () = msg_send![dock_layer, setShadowOffset: NSSize::new(0.0, -6.0)];
    let dock_shadow = color(0.0, 0.0, 0.0, 0.75);
    let dock_shadow_cg: *mut CGColor = msg_send![dock_shadow, CGColor];
    let _: () = msg_send![dock_layer, setShadowColor: dock_shadow_cg];

    for (frame, icon_layout) in frames.iter().zip(layout.dock_icons.iter()) {
        let icon_global = appkit_rect(*icon_layout, bounds_height);
        let icon_frame = NSRect::new(
            NSPoint::new(
                icon_global.origin.x - dock_frame.origin.x,
                icon_global.origin.y - dock_frame.origin.y,
            ),
            icon_global.size,
        );
        let (_, icon, fallback) = target_identity(frame);
        if icon.is_null() {
            let tile = rounded_view(
                icon_frame,
                icon_frame.size.width * 0.22,
                color(0.98, 0.98, 0.99, 0.86),
                true,
            );
            add_text_label(
                tile,
                NSRect::new(
                    NSPoint::new(0.0, (icon_frame.size.height - 24.0) / 2.0),
                    NSSize::new(icon_frame.size.width, 24.0),
                ),
                &fallback.to_string(),
                (icon_frame.size.width * 0.46).clamp(12.0, 22.0),
                true,
                LabelTone::Dark,
                1,
            );
            let _: () = msg_send![dock, addSubview: tile];
        } else {
            let image_view: *mut AnyObject = {
                let alloc: *mut AnyObject = msg_send![class!(NSImageView), alloc];
                msg_send![
                    alloc,
                    initWithFrame: NSRect::new(
                        icon_frame.origin,
                        icon_frame.size,
                    )
                ]
            };
            let _: () = msg_send![image_view, setImageScaling: 3u64];
            let _: () = msg_send![image_view, setImage: icon];
            let _: () = msg_send![dock, addSubview: image_view];
        }

        let dot_size = 3.2_f64.min(icon_frame.size.width * 0.10);
        let dot_x = icon_frame.origin.x + (icon_frame.size.width - dot_size) / 2.0;
        add_circle(dock, dot_x, 2.2, dot_size, color(0.20, 0.72, 0.38, 0.92));
    }
    let _: () = msg_send![canvas, addSubview: dock];
}

fn session_tabs_layout(
    width: f64,
    height: f64,
    workspaces: &[PipWorkspaceSummary],
    selected_workspace_id: Option<&str>,
) -> SessionTabsLayout {
    layout_session_tabs(
        LayoutRect {
            x: (width - 600.0).max(0.0) / 2.0,
            y: (height - SESSION_SELECTOR_HEIGHT).max(0.0),
            width: width.min(600.0),
            height: SESSION_SELECTOR_HEIGHT,
        },
        workspaces,
        selected_workspace_id,
    )
}

unsafe fn render_session_selector(
    canvas: *mut objc2::runtime::AnyObject,
    bounds: objc2_foundation::NSRect,
    workspaces: &[PipWorkspaceSummary],
    selected_workspace_id: Option<&str>,
) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    let tabs = session_tabs_layout(
        bounds.size.width,
        bounds.size.height,
        workspaces,
        selected_workspace_id,
    );
    let Some(first) = tabs.tabs.first() else {
        return;
    };
    let last = tabs.tabs.last().expect("tabs are non-empty");
    let selector_frame = objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(first.rect.x - 6.0, first.rect.y - 3.0),
        objc2_foundation::NSSize::new(
            last.rect.x + last.rect.width - first.rect.x + 12.0,
            first.rect.height + 6.0,
        ),
    );
    let selector = visual_effect_view(selector_frame, 14.0, 6, 1, true);
    let selector_layer: *mut AnyObject = msg_send![selector, layer];
    set_layer_background(selector_layer, color(0.10, 0.12, 0.15, 0.30));
    set_layer_border(selector_layer, 0.6, color(1.0, 1.0, 1.0, 0.26));

    let delegate = HANDLES
        .lock()
        .unwrap()
        .as_ref()
        .map(|handles| handles.delegate as *mut AnyObject)
        .unwrap_or(std::ptr::null_mut());
    for (index, tab) in tabs.tabs.iter().enumerate() {
        let workspace = &workspaces[index];
        let selected = tab.selected;
        let (red, green, blue) = (
            f64::from(tab.accent.0) / 255.0,
            f64::from(tab.accent.1) / 255.0,
            f64::from(tab.accent.2) / 255.0,
        );
        let icon_frame = objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(tab.rect.x, tab.rect.y),
            objc2_foundation::NSSize::new(tab.rect.width, tab.rect.height),
        );
        let button: *mut AnyObject = {
            let allocated: *mut AnyObject = msg_send![class!(NSButton), alloc];
            msg_send![allocated, initWithFrame: icon_frame]
        };
        let _: () = msg_send![button, setBordered: false];
        let _: () = msg_send![button, setRefusesFirstResponder: true];
        let _: () = msg_send![button, setWantsLayer: true];
        let layer: *mut AnyObject = msg_send![button, layer];
        let _: () = msg_send![layer, setCornerRadius: 11.0_f64];
        set_continuous_corners(layer);
        set_layer_background(
            layer,
            color(red, green, blue, if selected { 0.96 } else { 0.62 }),
        );
        set_layer_border(
            layer,
            if selected { 1.4 } else { 0.5 },
            color(1.0, 1.0, 1.0, if selected { 0.94 } else { 0.38 }),
        );
        let initial = workspace
            .workspace_label
            .chars()
            .find(|character| character.is_alphanumeric())
            .unwrap_or('A')
            .to_uppercase()
            .collect::<String>();
        let title = ns_string(&initial);
        let tooltip = ns_string(&workspace.workspace_label);
        if !title.is_null() {
            let _: () = msg_send![button, setTitle: title];
        }
        if !tooltip.is_null() {
            let _: () = msg_send![button, setToolTip: tooltip];
        }
        let font: *mut AnyObject = msg_send![class!(NSFont), boldSystemFontOfSize: 10.0_f64];
        let _: () = msg_send![button, setFont: font];
        let _: () = msg_send![button, setTag: index as isize];
        if !delegate.is_null() {
            let _: () = msg_send![button, setTarget: delegate];
            let _: () = msg_send![button, setAction: objc2::sel!(selectWorkspace:)];
        }
        let _: () = msg_send![selector, addSubview: button];
    }
    let _: () = msg_send![canvas, addSubview: selector];
}

fn desktop_frame(bounds: objc2_foundation::NSRect) -> objc2_foundation::NSRect {
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    if !SHOW_SHELL_BORDER {
        return bounds;
    }

    NSRect::new(
        NSPoint::new(SHELL_SIDE_INSET, SHELL_BOTTOM_INSET),
        NSSize::new(
            (bounds.size.width - 2.0 * SHELL_SIDE_INSET).max(1.0),
            (bounds.size.height - SHELL_TOP_INSET - SHELL_BOTTOM_INSET).max(1.0),
        ),
    )
}

unsafe fn install_shell_details(
    content_view: *mut objc2::runtime::AnyObject,
    bounds: objc2_foundation::NSRect,
) {
    use objc2::msg_send;
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    if !SHOW_SHELL_BORDER {
        return;
    }

    let grip_width = 24.0;
    let grip = rounded_view(
        NSRect::new(
            NSPoint::new(
                (bounds.size.width - grip_width) / 2.0,
                bounds.size.height - 9.0,
            ),
            NSSize::new(grip_width, 3.0),
        ),
        1.5,
        color(0.08, 0.10, 0.13, 0.52),
        true,
    );
    // Keep the grip centred and pinned to the shell's top edge while resizing.
    let _: () = msg_send![grip, setAutoresizingMask: 13u64];
    let grip_layer: *mut objc2::runtime::AnyObject = msg_send![grip, layer];
    let _: () = msg_send![grip_layer, setShadowOpacity: 0.32_f32];
    let _: () = msg_send![grip_layer, setShadowRadius: 0.8_f64];
    let _: () = msg_send![grip_layer, setShadowOffset: NSSize::new(0.0, 1.0)];
    let _: () = msg_send![content_view, addSubview: grip];

    let marker = color(1.0, 1.0, 1.0, 0.48);
    for (index, length) in [10.0_f64, 6.0].into_iter().enumerate() {
        let bar = rounded_view(
            NSRect::new(
                NSPoint::new(
                    (bounds.size.width - 16.0 + index as f64 * 4.0).max(0.0),
                    4.5 + index as f64 * 2.0,
                ),
                NSSize::new(length, 1.2),
            ),
            0.6,
            marker,
            false,
        );
        let _: () = msg_send![bar, setFrameCenterRotation: -45.0_f64];
        // Move with the right edge while remaining in the bottom shell rail.
        let _: () = msg_send![bar, setAutoresizingMask: 1u64];
        let _: () = msg_send![content_view, addSubview: bar];
    }
}

fn resized_window_frame(
    start: objc2_foundation::NSRect,
    delta_x: f64,
    delta_y: f64,
    direction: isize,
    minimum: objc2_foundation::NSSize,
) -> objc2_foundation::NSRect {
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let mut x = start.origin.x;
    let mut y = start.origin.y;
    let mut width = start.size.width;
    let mut height = start.size.height;
    if direction & RESIZE_LEFT != 0 {
        let applied = delta_x.min(width - minimum.width);
        x += applied;
        width -= applied;
    }
    if direction & RESIZE_RIGHT != 0 {
        width = (width + delta_x).max(minimum.width);
    }
    if direction & RESIZE_BOTTOM != 0 {
        let applied = delta_y.min(height - minimum.height);
        y += applied;
        height -= applied;
    }
    if direction & RESIZE_TOP != 0 {
        height = (height + delta_y).max(minimum.height);
    }
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

fn agent_view_shell_hit_class() -> &'static objc2::runtime::AnyClass {
    use objc2::class;
    use objc2::declare::ClassBuilder;

    static CLASS: OnceLock<&'static objc2::runtime::AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut builder = ClassBuilder::new("CuaDriverAgentViewShellHitView", class!(NSView))
            .expect("CuaDriverAgentViewShellHitView already registered");
        unsafe {
            builder.add_method(
                objc2::sel!(acceptsFirstMouse:),
                shell_accepts_first_mouse as extern "C" fn(_, _, _) -> objc2::runtime::Bool,
            );
            builder.add_method(
                objc2::sel!(mouseDown:),
                shell_mouse_down as extern "C" fn(_, _, _),
            );
        }
        builder.register()
    })
}

extern "C" fn shell_accepts_first_mouse(
    _view: *mut objc2::runtime::AnyObject,
    _selector: objc2::runtime::Sel,
    _event: *mut objc2::runtime::AnyObject,
) -> objc2::runtime::Bool {
    objc2::runtime::Bool::YES
}

extern "C" fn shell_mouse_down(
    view: *mut objc2::runtime::AnyObject,
    _selector: objc2::runtime::Sel,
    event: *mut objc2::runtime::AnyObject,
) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    if view.is_null() || event.is_null() {
        return;
    }
    unsafe {
        let window: *mut AnyObject = msg_send![view, window];
        if window.is_null() {
            return;
        }
        let direction = rust_string(msg_send![view, toolTip])
            .and_then(|value| value.parse::<isize>().ok())
            .unwrap_or(0);
        if direction == 0 {
            let _: () = msg_send![window, performWindowDragWithEvent: event];
            return;
        }

        let start_frame: objc2_foundation::NSRect = msg_send![window, frame];
        let minimum: objc2_foundation::NSSize = msg_send![window, minSize];
        let start_mouse: objc2_foundation::NSPoint =
            msg_send![objc2::class!(NSEvent), mouseLocation];
        let event_mask: u64 = (1 << 2) | (1 << 6);
        let distant_future: *mut AnyObject = msg_send![objc2::class!(NSDate), distantFuture];
        let default_mode = ns_string("kCFRunLoopDefaultMode");
        loop {
            let next: *mut AnyObject = msg_send![
                window,
                nextEventMatchingMask: event_mask
                untilDate: distant_future
                inMode: default_mode
                dequeue: true
            ];
            if next.is_null() {
                break;
            }
            let event_type: usize = msg_send![next, type];
            if event_type == 2 {
                break;
            }
            let mouse: objc2_foundation::NSPoint = msg_send![objc2::class!(NSEvent), mouseLocation];
            let frame = resized_window_frame(
                start_frame,
                mouse.x - start_mouse.x,
                mouse.y - start_mouse.y,
                direction,
                minimum,
            );
            let _: () = msg_send![window, setFrame: frame display: true];
        }
    }
}

unsafe fn add_shell_hit_view(
    parent: *mut objc2::runtime::AnyObject,
    frame: objc2_foundation::NSRect,
    direction: isize,
    autoresizing_mask: u64,
) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    let allocated: *mut AnyObject = msg_send![agent_view_shell_hit_class(), alloc];
    let view: *mut AnyObject = msg_send![allocated, initWithFrame: frame];
    let direction_string = ns_string(&direction.to_string());
    let _: () = msg_send![view, setToolTip: direction_string];
    let _: () = msg_send![view, setAutoresizingMask: autoresizing_mask];
    let _: () = msg_send![parent, addSubview: view];
}

unsafe fn install_shell_interaction(
    content_view: *mut objc2::runtime::AnyObject,
    bounds: objc2_foundation::NSRect,
) {
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let edge = RESIZE_HIT_INSET;
    let width = bounds.size.width;
    let height = bounds.size.height;
    add_shell_hit_view(
        content_view,
        NSRect::new(
            NSPoint::new(edge, height - edge),
            NSSize::new(width - 2.0 * edge, edge),
        ),
        RESIZE_TOP,
        10,
    );
    add_shell_hit_view(
        content_view,
        NSRect::new(
            NSPoint::new(edge, 0.0),
            NSSize::new(width - 2.0 * edge, edge),
        ),
        RESIZE_BOTTOM,
        10,
    );
    add_shell_hit_view(
        content_view,
        NSRect::new(
            NSPoint::new(0.0, edge),
            NSSize::new(edge, height - 2.0 * edge),
        ),
        RESIZE_LEFT,
        20,
    );
    add_shell_hit_view(
        content_view,
        NSRect::new(
            NSPoint::new(width - edge, edge),
            NSSize::new(edge, height - 2.0 * edge),
        ),
        RESIZE_RIGHT,
        17,
    );
    for (origin, direction, mask) in [
        (NSPoint::new(0.0, 0.0), RESIZE_LEFT | RESIZE_BOTTOM, 4),
        (
            NSPoint::new(width - edge, 0.0),
            RESIZE_RIGHT | RESIZE_BOTTOM,
            1,
        ),
        (
            NSPoint::new(0.0, height - edge),
            RESIZE_LEFT | RESIZE_TOP,
            8,
        ),
        (
            NSPoint::new(width - edge, height - edge),
            RESIZE_RIGHT | RESIZE_TOP,
            2,
        ),
    ] {
        add_shell_hit_view(
            content_view,
            NSRect::new(origin, NSSize::new(edge, edge)),
            direction,
            mask,
        );
    }
    add_shell_hit_view(
        content_view,
        NSRect::new(
            NSPoint::new(edge, height - SHELL_TOP_INSET),
            NSSize::new(width - 2.0 * edge, SHELL_TOP_INSET - edge),
        ),
        0,
        10,
    );
}

unsafe fn render_snapshot(snapshot: &ViewSnapshot) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let canvas = {
        let guard = HANDLES.lock().unwrap();
        match guard.as_ref() {
            Some(handles) => handles.canvas_view as *mut AnyObject,
            None => return,
        }
    };
    let empty: *mut AnyObject = msg_send![class!(NSArray), array];
    let _: () = msg_send![canvas, setSubviews: empty];

    let bounds: NSRect = msg_send![canvas, bounds];
    let selector_height = if snapshot.workspaces.len() > 1 {
        SESSION_SELECTOR_HEIGHT
    } else {
        0.0
    };
    let content_height = (bounds.size.height - selector_height).max(1.0);
    let target_sizes = snapshot
        .frames
        .iter()
        .map(|frame| {
            png_dimensions(&frame.png_bytes).unwrap_or(TargetSize {
                width: 16,
                height: 10,
            })
        })
        .collect::<Vec<_>>();
    let layout = layout_desktop(bounds.size.width, content_height, &target_sizes);
    if snapshot.frames.is_empty() {
        let waiting = appkit_rect(layout.desktop, content_height);
        add_text_label(
            canvas,
            NSRect::new(
                NSPoint::new(
                    waiting.origin.x + 16.0,
                    waiting.origin.y + waiting.size.height / 2.0 - 12.0,
                ),
                NSSize::new((waiting.size.width - 32.0).max(40.0), 24.0),
            ),
            "Waiting for an exact window or browser tab...",
            12.0,
            false,
            LabelTone::Muted,
            1,
        );
    } else {
        for (frame, target_layout) in snapshot.frames.iter().zip(layout.targets.iter().copied()) {
            render_target_window(
                canvas,
                frame,
                target_layout,
                content_height,
                snapshot.active_view_id.as_deref() == Some(frame.target.view_id().as_str()),
            );
        }
        // Keep the resting Agent View free of a redundant internal Dock.
    }
    render_session_selector(
        canvas,
        bounds,
        &snapshot.workspaces,
        snapshot.selected_workspace_id.as_deref(),
    );
}

unsafe extern "C" fn shutdown_cb(_ctx: *mut c_void) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    let handles = HANDLES.lock().unwrap().take();
    VIEW_MODEL.lock().unwrap().take();
    if let Some(handles) = handles {
        let window = handles.window as *mut AnyObject;
        let _: () = msg_send![window, setDelegate: std::ptr::null_mut::<AnyObject>()];
        let _: () = msg_send![window, orderOut: std::ptr::null_mut::<AnyObject>()];
        let _: () = msg_send![window, close];
        let _ = handles.delegate;
    }
}

fn agent_view_delegate_class() -> &'static objc2::runtime::AnyClass {
    use objc2::class;
    use objc2::declare::ClassBuilder;

    static CLASS: OnceLock<&'static objc2::runtime::AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let superclass = class!(NSObject);
        let mut builder = ClassBuilder::new("CuaDriverAgentViewDelegate", superclass)
            .expect("CuaDriverAgentViewDelegate already registered");
        unsafe {
            builder.add_method(
                objc2::sel!(windowDidResize:),
                on_window_did_resize as extern "C" fn(_, _, _),
            );
            builder.add_method(
                objc2::sel!(selectWorkspace:),
                on_select_workspace as extern "C" fn(_, _, _),
            );
        }
        builder.register()
    })
}

unsafe fn agent_view_delegate_instance() -> *mut objc2::runtime::AnyObject {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    let class = agent_view_delegate_class();
    let allocated: *mut AnyObject = msg_send![class, alloc];
    msg_send![allocated, init]
}

extern "C" fn on_window_did_resize(
    _delegate: *mut objc2::runtime::AnyObject,
    _selector: objc2::runtime::Sel,
    _notification: *mut objc2::runtime::AnyObject,
) {
    let snapshot = current_snapshot();
    unsafe {
        update_wallpaper_frame();
        render_snapshot(&snapshot);
    };
}

extern "C" fn on_select_workspace(
    _delegate: *mut objc2::runtime::AnyObject,
    _selector: objc2::runtime::Sel,
    sender: *mut objc2::runtime::AnyObject,
) {
    if sender.is_null() {
        return;
    }
    let index: isize = unsafe { objc2::msg_send![sender, tag] };
    if index < 0 {
        return;
    }
    let snapshot = {
        let mut model = VIEW_MODEL.lock().unwrap();
        let Some(model) = model.as_mut() else {
            return;
        };
        let workspaces = model.workspaces();
        let Some(workspace) = workspaces.get(index as usize) else {
            return;
        };
        let workspace_id = workspace.workspace_id.clone();
        model.select_workspace(&workspace_id);
        clone_snapshot(model)
    };
    unsafe { render_snapshot(&snapshot) };
}

/// Park the main thread in `NSApplication.run()` for the main-queue AppKit
/// work used by Agent View when the cursor overlay is disabled.
pub fn run_appkit_main_loop() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    let _mtm = objc2_foundation::MainThreadMarker::new()
        .expect("run_appkit_main_loop must be called from the main thread");
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: bool = msg_send![app, setActivationPolicy: 1i64];
        let _: () = msg_send![app, finishLaunching];
        let _: () = msg_send![app, run];
    }
}

pub struct MacosPipBackendFactory;

impl PipBackendFactory for MacosPipBackendFactory {
    fn start(&self, cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>> {
        dispatch_to_main(cfg.clone(), init_cb);
        Ok(Box::new(MacosPipBackend))
    }
}

unsafe fn install_wallpaper(
    content_view: *mut objc2::runtime::AnyObject,
    screen: *mut objc2::runtime::AnyObject,
    bounds: objc2_foundation::NSRect,
) -> (
    *mut objc2::runtime::AnyObject,
    *mut objc2::runtime::AnyObject,
    *mut objc2::runtime::AnyObject,
    *mut objc2::runtime::AnyObject,
) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    // The same wallpaper continues beneath the shell rails. The centre is
    // covered by a sharp inset copy while the outer copy is blurred and tinted.
    let glass_frame = rounded_view(
        bounds,
        if SHOW_SHELL_BORDER {
            CONTAINER_RADIUS
        } else {
            PIP_CORNER_RADIUS
        },
        color(0.18, 0.21, 0.25, if SHOW_SHELL_BORDER { 0.16 } else { 0.0 }),
        true,
    );
    let _: () = msg_send![glass_frame, setAutoresizingMask: 18u64];
    let glass_layer: *mut AnyObject = msg_send![glass_frame, layer];
    set_continuous_corners(glass_layer);
    if SHOW_SHELL_BORDER {
        set_layer_border(glass_layer, 0.8, color(1.0, 1.0, 1.0, 0.50));
    }
    let _: () = msg_send![content_view, addSubview: glass_frame];

    let glass_bounds: objc2_foundation::NSRect = msg_send![glass_frame, bounds];
    let shell_wallpaper_view: *mut AnyObject = {
        let allocated: *mut AnyObject = msg_send![class!(NSImageView), alloc];
        msg_send![allocated, initWithFrame: glass_bounds]
    };
    let _: () = msg_send![shell_wallpaper_view, setImageScaling: 2u64];
    if SHOW_SHELL_BORDER {
        let _: () = msg_send![glass_frame, addSubview: shell_wallpaper_view];
    }

    let backdrop: *mut AnyObject = {
        let allocated: *mut AnyObject = msg_send![class!(NSVisualEffectView), alloc];
        msg_send![
            allocated,
            initWithFrame: objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(0.0, 0.0),
                bounds.size,
            )
        ]
    };
    let _: () = msg_send![backdrop, setAutoresizingMask: 18u64];
    let _: () = msg_send![backdrop, setMaterial: 13i64]; // HUD window material.
    let _: () = msg_send![backdrop, setBlendingMode: 1i64]; // Blur the wallpaper within the shell.
    let _: () = msg_send![backdrop, setState: 1i64]; // Keep the blur active while non-key.
    let _: () = msg_send![backdrop, setEmphasized: true];
    if SHOW_SHELL_BORDER {
        let _: () = msg_send![glass_frame, addSubview: backdrop];
    }

    // A restrained silver-to-graphite tint keeps the blurred backdrop legible
    // as a shell instead of exposing raw, high-contrast shapes behind it.
    let body = gradient_view(
        objc2_foundation::NSRect::new(objc2_foundation::NSPoint::new(0.0, 0.0), bounds.size),
        CONTAINER_RADIUS,
        color(0.68, 0.71, 0.76, 0.34),
        color(0.15, 0.18, 0.22, 0.52),
    );
    let _: () = msg_send![body, setAutoresizingMask: 18u64];
    let body_layer: *mut AnyObject = msg_send![body, layer];
    let scale: f64 = msg_send![screen, backingScaleFactor];
    if scale > 0.0 {
        // Layer-hosting views do not inherit the backing scale, and the chrome
        // hairlines are sub-point.
        let _: () = msg_send![body_layer, setContentsScale: scale];
    }
    if SHOW_SHELL_BORDER {
        let _: () = msg_send![glass_frame, addSubview: body];
    }

    // Light-from-above specular along the top edge of the chrome.
    let highlight = rounded_view(
        objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(CONTAINER_RADIUS, bounds.size.height - 1.7),
            objc2_foundation::NSSize::new(
                (bounds.size.width - 2.0 * CONTAINER_RADIUS).max(1.0),
                1.0,
            ),
        ),
        0.5,
        color(1.0, 1.0, 1.0, 0.38),
        true,
    );
    let _: () = msg_send![highlight, setAutoresizingMask: 10u64];
    if SHOW_SHELL_BORDER {
        let _: () = msg_send![glass_frame, addSubview: highlight];
    }

    let container_frame = desktop_frame(bounds);
    let container_radius = if SHOW_SHELL_BORDER {
        8.5
    } else {
        PIP_CORNER_RADIUS
    };
    let wallpaper_shadow = rounded_view(
        container_frame,
        container_radius,
        color(0.0, 0.0, 0.0, 0.01),
        false,
    );
    let _: () = msg_send![wallpaper_shadow, setAutoresizingMask: 18u64];
    let shadow_layer: *mut AnyObject = msg_send![wallpaper_shadow, layer];
    set_continuous_corners(shadow_layer);
    // The inset shadow separates the desktop from the thicker shell rails.
    let _: () = msg_send![shadow_layer, setShadowOpacity: 0.62_f32];
    let _: () = msg_send![shadow_layer, setShadowRadius: 4.0_f64];
    let _: () = msg_send![shadow_layer, setShadowOffset: objc2_foundation::NSSize::new(0.0, -1.5)];
    let shadow_color = color(0.0, 0.0, 0.0, 0.85);
    let shadow_cg: *mut CGColor = msg_send![shadow_color, CGColor];
    let _: () = msg_send![shadow_layer, setShadowColor: shadow_cg];
    if SHOW_SHELL_BORDER {
        let _: () = msg_send![content_view, addSubview: wallpaper_shadow];
    }

    // Machined inner edge. It hugs the seam from the chrome side, so the
    // miniature desktop stays separated from the graphite body whether the
    // content under it is a bright wallpaper or a dark window.
    let seam_outset = 0.75;
    let seam = rounded_view(
        objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(
                container_frame.origin.x - seam_outset,
                container_frame.origin.y - seam_outset,
            ),
            objc2_foundation::NSSize::new(
                container_frame.size.width + 2.0 * seam_outset,
                container_frame.size.height + 2.0 * seam_outset,
            ),
        ),
        container_radius + seam_outset,
        color(0.0, 0.0, 0.0, 0.0),
        false,
    );
    let _: () = msg_send![seam, setAutoresizingMask: 18u64];
    let seam_layer: *mut AnyObject = msg_send![seam, layer];
    set_continuous_corners(seam_layer);
    if SHOW_SHELL_BORDER {
        set_layer_border(seam_layer, seam_outset, color(1.0, 1.0, 1.0, 0.20));
    }
    let _: () = msg_send![content_view, addSubview: seam];

    let wallpaper_container = rounded_view(
        container_frame,
        container_radius,
        color(0.02, 0.03, 0.04, if SHOW_SHELL_BORDER { 0.68 } else { 0.0 }),
        true,
    );
    let _: () = msg_send![wallpaper_container, setAutoresizingMask: 18u64];
    let container_layer: *mut AnyObject = msg_send![wallpaper_container, layer];
    set_continuous_corners(container_layer);
    // Dark hairline where the miniature desktop meets the chrome. Softer than
    // the flat-outline pass, because the graphite body now carries the seam.
    if SHOW_SHELL_BORDER {
        set_layer_border(container_layer, 0.8, color(0.0, 0.0, 0.0, 0.30));
    }
    let container_bounds: objc2_foundation::NSRect = msg_send![wallpaper_container, bounds];
    let wallpaper_view: *mut AnyObject = {
        let allocated: *mut AnyObject = msg_send![class!(NSImageView), alloc];
        msg_send![allocated, initWithFrame: container_bounds]
    };
    let _: () = msg_send![wallpaper_view, setImageScaling: 2u64];
    let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
    let url: *mut AnyObject = msg_send![workspace, desktopImageURLForScreen: screen];
    if !url.is_null() {
        let allocated: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let wallpaper: *mut AnyObject = msg_send![allocated, initWithContentsOfURL: url];
        if !wallpaper.is_null() {
            let _: () = msg_send![shell_wallpaper_view, setImage: wallpaper];
            let _: () = msg_send![wallpaper_view, setImage: wallpaper];
        }
    }
    let _: () = msg_send![wallpaper_container, addSubview: wallpaper_view];
    let _: () = msg_send![content_view, addSubview: wallpaper_container];

    (
        glass_frame,
        shell_wallpaper_view,
        wallpaper_container,
        wallpaper_view,
    )
}

fn aspect_fill_frame(
    bounds: objc2_foundation::NSRect,
    image_size: objc2_foundation::NSSize,
) -> objc2_foundation::NSRect {
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    if image_size.width <= 0.0 || image_size.height <= 0.0 {
        return bounds;
    }
    let scale = (bounds.size.width / image_size.width).max(bounds.size.height / image_size.height);
    let width = image_size.width * scale;
    let height = image_size.height * scale;
    NSRect::new(
        NSPoint::new(
            bounds.origin.x + (bounds.size.width - width) / 2.0,
            bounds.origin.y + (bounds.size.height - height) / 2.0,
        ),
        NSSize::new(width, height),
    )
}

unsafe fn update_wallpaper_frame() {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSRect, NSSize};

    let (shell_container, shell_view, wallpaper_container, wallpaper_view) = {
        let guard = HANDLES.lock().unwrap();
        let Some(handles) = guard.as_ref() else {
            return;
        };
        (
            handles.shell_wallpaper_container as *mut AnyObject,
            handles.shell_wallpaper_view as *mut AnyObject,
            handles.wallpaper_container as *mut AnyObject,
            handles.wallpaper_view as *mut AnyObject,
        )
    };
    for (container, view) in [
        (shell_container, shell_view),
        (wallpaper_container, wallpaper_view),
    ] {
        let bounds: NSRect = msg_send![container, bounds];
        let image: *mut AnyObject = msg_send![view, image];
        let image_size = if image.is_null() {
            NSSize::new(0.0, 0.0)
        } else {
            msg_send![image, size]
        };
        let frame = aspect_fill_frame(bounds, image_size);
        let _: () = msg_send![view, setFrame: frame];
    }
}

unsafe extern "C" fn init_cb(ctx: *mut c_void) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let cfg: PipConfig = *Box::from_raw(ctx as *mut PipConfig);
    if HANDLES.lock().unwrap().is_some() {
        return;
    }

    let screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
    if screen.is_null() {
        return;
    }
    let screen_frame: NSRect = msg_send![screen, frame];
    let minimum = NSSize::new(360.0, 260.0);
    let width = (cfg.geometry.width as f64).max(minimum.width);
    let height = (cfg.geometry.height as f64).max(minimum.height);
    let inset = 24.0_f64;
    let (top_left_x, top_left_y) = match (cfg.geometry.x, cfg.geometry.y) {
        (Some(x), Some(y)) => (x as f64, y as f64),
        _ => (screen_frame.size.width - width - inset, inset),
    };
    let bottom_y = screen_frame.size.height - top_left_y - height;
    let rect = NSRect::new(
        NSPoint::new(top_left_x, bottom_y),
        NSSize::new(width, height),
    );

    let style_mask: u64 = (1 << 3) | (1 << 7);
    let backing_store_buffered: u64 = 2;
    let window: *mut AnyObject = {
        let allocated: *mut AnyObject = msg_send![class!(NSPanel), alloc];
        msg_send![
            allocated,
            initWithContentRect: rect
            styleMask: style_mask
            backing: backing_store_buffered
            defer: false
        ]
    };
    if window.is_null() {
        return;
    }

    let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
    let _: () = msg_send![window, setBackgroundColor: clear];
    let _: () = msg_send![window, setOpaque: false];
    let _: () = msg_send![window, setHasShadow: SHOW_SHELL_BORDER];
    // Screen sharing and recordings should include Agent View itself.
    let _: () = msg_send![window, setSharingType: 1u64];
    let _: () = msg_send![window, setMovableByWindowBackground: true];
    let _: () = msg_send![window, setIgnoresMouseEvents: false];
    let _: () = msg_send![window, setBecomesKeyOnlyIfNeeded: true];
    let _: () = msg_send![window, setFloatingPanel: true];
    let _: () = msg_send![window, setLevel: 3i64];
    let _: () = msg_send![window, setMinSize: minimum];
    let behavior: u64 = (1 << 0) | (1 << 4) | (1 << 8) | (1 << 6) | (1 << 7);
    let _: () = msg_send![window, setCollectionBehavior: behavior];
    let _: () = msg_send![window, setReleasedWhenClosed: false];
    let _: () = msg_send![window, setHidesOnDeactivate: false];

    let content_view: *mut AnyObject = msg_send![window, contentView];
    let _: () = msg_send![content_view, setWantsLayer: true];
    let content_layer: *mut AnyObject = msg_send![content_view, layer];
    let _: () = msg_send![
        content_layer,
        setCornerRadius: if SHOW_SHELL_BORDER {
            CONTAINER_RADIUS
        } else {
            PIP_CORNER_RADIUS
        }
    ];
    set_continuous_corners(content_layer);
    // The chrome view clips its own children. Masking here too would clip the
    // chrome's own outer rim stroke in half and flatten the frame.
    let _: () = msg_send![content_layer, setMasksToBounds: false];
    set_layer_background(content_layer, color(0.0, 0.0, 0.0, 0.0));
    let bounds: NSRect = msg_send![content_view, bounds];
    let (shell_wallpaper_container, shell_wallpaper_view, wallpaper_container, wallpaper_view) =
        install_wallpaper(content_view, screen, bounds);
    let inner_frame = desktop_frame(bounds);

    let canvas: *mut AnyObject = {
        let allocated: *mut AnyObject = msg_send![class!(NSView), alloc];
        msg_send![allocated, initWithFrame: inner_frame]
    };
    let _: () = msg_send![canvas, setAutoresizingMask: 18u64];
    // The content view no longer masks, so the canvas clips its own contents to
    // the container silhouette.
    let _: () = msg_send![canvas, setWantsLayer: true];
    let canvas_layer: *mut AnyObject = msg_send![canvas, layer];
    let _: () = msg_send![
        canvas_layer,
        setCornerRadius: if SHOW_SHELL_BORDER {
            8.5_f64
        } else {
            PIP_CORNER_RADIUS
        }
    ];
    set_continuous_corners(canvas_layer);
    let _: () = msg_send![canvas_layer, setMasksToBounds: true];
    let _: () = msg_send![content_view, addSubview: canvas];
    install_shell_details(content_view, bounds);
    install_shell_interaction(content_view, bounds);

    let delegate = agent_view_delegate_instance();
    let _: () = msg_send![window, setDelegate: delegate];
    *HANDLES.lock().unwrap() = Some(NativeHandles {
        window: window as usize,
        canvas_view: canvas as usize,
        shell_wallpaper_container: shell_wallpaper_container as usize,
        shell_wallpaper_view: shell_wallpaper_view as usize,
        wallpaper_container: wallpaper_container as usize,
        wallpaper_view: wallpaper_view as usize,
        delegate: delegate as usize,
    });
    *VIEW_MODEL.lock().unwrap() = Some(PipViewModel::new(12));
    update_wallpaper_frame();
    render_snapshot(&ViewSnapshot {
        frames: Vec::new(),
        workspaces: Vec::new(),
        selected_workspace_id: None,
        active_view_id: None,
    });
    let _: () = msg_send![window, orderFrontRegardless];

    tracing::info!(
        target: "pip",
        "Agent View miniature desktop initialised ({}x{})",
        width,
        height
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(id: &str, target_count: usize, updated_ms: u64) -> PipWorkspaceSummary {
        PipWorkspaceSummary {
            workspace_id: id.to_owned(),
            workspace_label: id.to_owned(),
            target_count,
            updated_ms,
        }
    }

    #[test]
    fn parses_native_pid_from_exact_window_target() {
        assert_eq!(parse_native_pid("window:501:90210"), Some(501));
        assert_eq!(parse_native_pid("browser:target:tab"), None);
        assert_eq!(parse_native_pid("window:not-a-pid:7"), None);
    }

    #[test]
    fn truncates_long_titles_without_touching_short_ones() {
        assert_eq!(truncate_label("Calculator", 28), "Calculator");
        assert_eq!(
            truncate_label("abcdefghijklmnopqrstuvwxyz", 8),
            "abcdefgh..."
        );
    }

    #[test]
    fn aspect_fill_centers_and_crops_without_distortion() {
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        let wide = aspect_fill_frame(
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 420.0)),
            NSSize::new(1440.0, 900.0),
        );
        assert_eq!(wide.size, NSSize::new(672.0, 420.0));
        assert_eq!(wide.origin, NSPoint::new(-16.0, 0.0));

        let tall = aspect_fill_frame(
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(380.0, 660.0)),
            NSSize::new(1440.0, 900.0),
        );
        assert_eq!(tall.size, NSSize::new(1056.0, 660.0));
        assert_eq!(tall.origin, NSPoint::new(-338.0, 0.0));
    }

    #[test]
    fn desktop_frame_uses_full_bounds_when_shell_border_is_hidden() {
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        let outer = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(620.0, 420.0));
        let inner = desktop_frame(outer);

        assert_eq!(inner, outer);
    }

    #[test]
    fn session_selector_is_local_compact_and_hidden_for_one_session() {
        let one = [workspace("one", 1, 1)];
        assert!(session_tabs_layout(602.0, 390.0, &one, Some("one"))
            .tabs
            .is_empty());
        let workspaces = [
            workspace("one", 1, 1),
            workspace("two", 1, 1),
            workspace("three", 1, 1),
        ];
        let tabs = session_tabs_layout(602.0, 390.0, &workspaces, Some("one"));
        assert_eq!(tabs.tabs.len(), 3);
        assert!(tabs.tabs.iter().all(|tab| {
            tab.rect.x >= 0.0
                && tab.rect.y >= 0.0
                && tab.rect.x + tab.rect.width <= 602.0
                && tab.rect.y + tab.rect.height <= 390.0
        }));
    }

    #[test]
    fn every_resize_direction_keeps_the_opposite_edges_anchored() {
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        let start = NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(600.0, 400.0));
        let minimum = NSSize::new(360.0, 260.0);
        let left = resized_window_frame(start, 40.0, 0.0, RESIZE_LEFT, minimum);
        assert_eq!(left.origin.x, 140.0);
        assert_eq!(left.size.width, 560.0);
        let right = resized_window_frame(start, 40.0, 0.0, RESIZE_RIGHT, minimum);
        assert_eq!(right.origin.x, 100.0);
        assert_eq!(right.size.width, 640.0);
        let bottom = resized_window_frame(start, 0.0, 30.0, RESIZE_BOTTOM, minimum);
        assert_eq!(bottom.origin.y, 130.0);
        assert_eq!(bottom.size.height, 370.0);
        let top = resized_window_frame(start, 0.0, 30.0, RESIZE_TOP, minimum);
        assert_eq!(top.origin.y, 100.0);
        assert_eq!(top.size.height, 430.0);
        let corner = resized_window_frame(start, -25.0, 35.0, RESIZE_LEFT | RESIZE_TOP, minimum);
        assert_eq!(corner.origin, NSPoint::new(75.0, 100.0));
        assert_eq!(corner.size, NSSize::new(625.0, 435.0));
    }

    #[test]
    fn resize_geometry_enforces_the_minimum_without_moving_far_edges() {
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        let start = NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(600.0, 400.0));
        let minimum = NSSize::new(360.0, 260.0);
        let frame = resized_window_frame(start, 500.0, 500.0, RESIZE_LEFT | RESIZE_BOTTOM, minimum);
        assert_eq!(frame.origin, NSPoint::new(340.0, 240.0));
        assert_eq!(frame.size, minimum);
    }
}
