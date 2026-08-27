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
    layout_desktop, png_dimensions, PipBackend, PipBackendFactory, PipConfig, PipFrame,
    PipTargetKind, PipViewModel, PipWorkspaceSummary, TargetSize,
};

#[repr(C)]
struct CGColor {
    _opaque: [u8; 0],
}

unsafe impl objc2::RefEncode for CGColor {
    const ENCODING_REF: objc2::Encoding =
        objc2::Encoding::Pointer(&objc2::Encoding::Struct("CGColor", &[]));
}

/// Adopt the +1 reference returned by an Objective-C `alloc`/`init` pair.
///
/// Dynamic Agent View frames build a fresh AppKit subtree. Keeping each local
/// +1 reference in `Retained` lets the containing view/image property take its
/// own reference, then balances ours at the end of the render pass. Without
/// this, removing the old subviews only released AppKit's reference and every
/// frame leaked the renderer's original ownership.
unsafe fn adopt_alloc_init(
    object: *mut objc2::runtime::AnyObject,
) -> Option<objc2::rc::Retained<objc2::runtime::AnyObject>> {
    objc2::rc::Retained::from_raw(object)
}

fn owned_object_ptr(
    object: &objc2::rc::Retained<objc2::runtime::AnyObject>,
) -> *mut objc2::runtime::AnyObject {
    objc2::rc::Retained::as_ptr(object).cast_mut()
}

struct NativeHandles {
    window: usize,
    canvas_view: usize,
    shell_wallpaper_container: usize,
    shell_wallpaper_view: usize,
    wallpaper_container: usize,
    wallpaper_view: usize,
    delegate: usize,
    expand_button: usize,
}

#[derive(Default)]
struct ExpansionState {
    compact_frame: Option<[f64; 4]>,
    expanded: bool,
    timer_generation: u64,
}

impl ExpansionState {
    fn begin_glance(&mut self, compact_frame: [f64; 4]) -> u64 {
        self.compact_frame = Some(compact_frame);
        self.expanded = true;
        self.timer_generation = self.timer_generation.wrapping_add(1);
        self.timer_generation
    }

    fn end_glance(&mut self) -> Option<[f64; 4]> {
        if !self.expanded {
            return None;
        }
        self.expanded = false;
        self.timer_generation = self.timer_generation.wrapping_add(1);
        self.compact_frame
    }

    fn end_glance_for_timer(&mut self, generation: u64) -> Option<[f64; 4]> {
        if !self.expanded || self.timer_generation != generation {
            return None;
        }
        self.end_glance()
    }

    fn reset(&mut self) {
        // Invalidate any queued timeout before dropping the saved frame.
        self.timer_generation = self.timer_generation.wrapping_add(1);
        self.compact_frame = None;
        self.expanded = false;
    }
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
const GLANCE_DURATION_SECONDS: u64 = 8;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

// Keep the PiP shell visually clean for now; invisible hit zones still provide
// dragging and resizing without exposing a native or custom frame.
const SHOW_SHELL_BORDER: bool = false;

const RESIZE_LEFT: isize = 1;
const RESIZE_RIGHT: isize = 2;
const RESIZE_BOTTOM: isize = 4;
const RESIZE_TOP: isize = 8;

static HANDLES: Mutex<Option<NativeHandles>> = Mutex::new(None);
static VIEW_MODEL: Mutex<Option<PipViewModel>> = Mutex::new(None);
static EXPANSION_STATE: Mutex<ExpansionState> = Mutex::new(ExpansionState {
    compact_frame: None,
    expanded: false,
    timer_generation: 0,
});

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
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn dispatch_after_f(
        when: u64,
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

const NS_APPLICATION_ACTIVATION_POLICY_ACCESSORY: isize = 1;

fn validate_accessory_policy_postcondition(
    set_attempt_succeeded: bool,
    effective_policy: isize,
) -> anyhow::Result<()> {
    if effective_policy == NS_APPLICATION_ACTIVATION_POLICY_ACCESSORY {
        return Ok(());
    }
    anyhow::bail!(
        "Agent View could not establish the accessory activation policy \
         (setActivationPolicy returned {set_attempt_succeeded}; effective policy is \
         {effective_policy})"
    )
}

fn validate_window_server_readiness(
    window_number: isize,
    appkit_visible: bool,
    window_server_on_screen: bool,
) -> anyhow::Result<u32> {
    let window_id = u32::try_from(window_number)
        .ok()
        .filter(|window_id| *window_id > 0)
        .ok_or_else(|| anyhow::anyhow!("Agent View NSPanel has no valid WindowServer number"))?;
    if !appkit_visible {
        anyhow::bail!("Agent View NSPanel is not visible in AppKit");
    }
    if !window_server_on_screen {
        anyhow::bail!(
            "Agent View NSPanel {window_id} is not present in the on-screen WindowServer list"
        );
    }
    Ok(window_id)
}

fn window_server_reports_on_screen(window_id: u32) -> bool {
    use core_graphics::window::{
        create_window_list, kCGNullWindowID, kCGWindowListExcludeDesktopElements,
        kCGWindowListOptionOnScreenOnly,
    };

    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    create_window_list(options, kCGNullWindowID)
        .is_some_and(|windows| windows.iter().any(|candidate| *candidate == window_id))
}

pub struct MacosPipBackend {
    background_only: bool,
}

fn should_yield_native_hit_testing(background_only: bool) -> bool {
    !background_only
}

struct InputPassthroughRequest {
    passthrough: bool,
    applied: Arc<AtomicBool>,
}

impl PipBackend for MacosPipBackend {
    fn push_frame(&self, frame: PipFrame) {
        // Startup publishes the native handles synchronously. Frames can then
        // be rendered asynchronously without a queued-init ordering window.
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
        if !should_yield_native_hit_testing(self.background_only) {
            // Strict mode admits only semantic accessibility input and exact
            // process/window-routed events. Neither route uses the global
            // WindowServer hit test, so making this panel transparent is both
            // unnecessary and unsafe: a person's concurrent Expand/Shrink
            // click would fall through to the application below it.
            return Ok(());
        }
        let applied = Arc::new(AtomicBool::new(false));
        dispatch_to_main_sync(
            InputPassthroughRequest {
                passthrough,
                applied: Arc::clone(&applied),
            },
            set_input_passthrough_cb,
        );
        if !applied.load(Ordering::Acquire) {
            anyhow::bail!("Agent View window is not running");
        }
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
}

fn clone_snapshot(model: &PipViewModel) -> ViewSnapshot {
    ViewSnapshot {
        frames: model.selected_frames().into_iter().cloned().collect(),
        workspaces: model.workspaces(),
        selected_workspace_id: model.selected_workspace_id().map(str::to_owned),
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

    let Some(label_owner) = adopt_alloc_init({
        let alloc: *mut AnyObject = msg_send![class!(NSTextField), alloc];
        msg_send![alloc, initWithFrame: frame]
    }) else {
        return;
    };
    let label = owned_object_ptr(&label_owner);
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
    let Some(layer_owner) = adopt_alloc_init({
        let alloc: *mut AnyObject = msg_send![class!(CAGradientLayer), alloc];
        msg_send![alloc, init]
    }) else {
        let _: () = msg_send![view, release];
        return std::ptr::null_mut();
    };
    let layer = owned_object_ptr(&layer_owner);
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

    let Some(circle_owner) = adopt_alloc_init(rounded_view(
        NSRect::new(NSPoint::new(x, y), NSSize::new(diameter, diameter)),
        diameter / 2.0,
        fill,
        true,
    )) else {
        return;
    };
    let circle = owned_object_ptr(&circle_owner);
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
) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let window_frame = appkit_rect(layout.window, bounds_height);
    let radius = (window_frame.size.width.min(window_frame.size.height) * 0.035).clamp(5.0, 9.0);
    let Some(shadow_owner) = adopt_alloc_init(rounded_view(
        window_frame,
        radius,
        color(0.0, 0.0, 0.0, 0.0),
        false,
    )) else {
        return;
    };
    let shadow = owned_object_ptr(&shadow_owner);
    let shadow_layer: *mut AnyObject = msg_send![shadow, layer];
    let _: () = msg_send![shadow_layer, setShadowOpacity: 0.30_f32];
    let _: () = msg_send![shadow_layer, setShadowRadius: 16.0_f64];
    let _: () = msg_send![shadow_layer, setShadowOffset: NSSize::new(0.0, -6.0)];
    let black = color(0.0, 0.0, 0.0, 0.90);
    let black_cg: *mut CGColor = msg_send![black, CGColor];
    let _: () = msg_send![shadow_layer, setShadowColor: black_cg];

    let Some(window_owner) = adopt_alloc_init(rounded_view(
        NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(window_frame.size.width, window_frame.size.height),
        ),
        radius,
        color(0.0, 0.0, 0.0, 0.0),
        true,
    )) else {
        return;
    };
    let window = owned_object_ptr(&window_owner);
    let window_layer: *mut AnyObject = msg_send![window, layer];
    if SHOW_SHELL_BORDER {
        set_layer_border(window_layer, 0.55, color(0.0, 0.0, 0.0, 0.22));
    }
    let Some(image_view_owner) = adopt_alloc_init({
        let alloc: *mut AnyObject = msg_send![class!(NSImageView), alloc];
        msg_send![
            alloc,
            initWithFrame: NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(window_frame.size.width, window_frame.size.height),
            )
        ]
    }) else {
        return;
    };
    let image_view = owned_object_ptr(&image_view_owner);
    let _: () = msg_send![image_view, setImageScaling: 3u64];
    if let Some(image_owner) = adopt_alloc_init(image_from_png(&frame.png_bytes)) {
        let image = owned_object_ptr(&image_owner);
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
        let Some(pointer_owner) = adopt_alloc_init({
            let alloc: *mut AnyObject = msg_send![class!(NSImageView), alloc];
            msg_send![
                alloc,
                initWithFrame: NSRect::new(NSPoint::new(x, y), pointer_size)
            ]
        }) else {
            return;
        };
        let pointer = owned_object_ptr(&pointer_owner);
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
    let Some(dock_owner) = adopt_alloc_init(visual_effect_view(
        dock_frame,
        dock_frame.size.height * 0.30,
        6,
        1,
        true,
    )) else {
        return;
    };
    let dock = owned_object_ptr(&dock_owner);
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
            let Some(tile_owner) = adopt_alloc_init(rounded_view(
                icon_frame,
                icon_frame.size.width * 0.22,
                color(0.98, 0.98, 0.99, 0.86),
                true,
            )) else {
                continue;
            };
            let tile = owned_object_ptr(&tile_owner);
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
            let Some(image_view_owner) = adopt_alloc_init({
                let alloc: *mut AnyObject = msg_send![class!(NSImageView), alloc];
                msg_send![
                    alloc,
                    initWithFrame: NSRect::new(
                        icon_frame.origin,
                        icon_frame.size,
                    )
                ]
            }) else {
                continue;
            };
            let image_view = owned_object_ptr(&image_view_owner);
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

fn session_selector_layout(
    width: f64,
    height: f64,
    count: usize,
) -> Option<(objc2_foundation::NSRect, Vec<objc2_foundation::NSRect>)> {
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    if count <= 1 {
        return None;
    }
    let visible = count;
    let icon = 22.0;
    let gap = 5.0;
    let padding = 6.0;
    let selector_width = padding * 2.0 + visible as f64 * icon + (visible - 1) as f64 * gap;
    let selector = NSRect::new(
        NSPoint::new((width - selector_width) / 2.0, height - 29.0),
        NSSize::new(selector_width, 28.0),
    );
    let icons = (0..visible)
        .map(|index| {
            NSRect::new(
                NSPoint::new(padding + index as f64 * (icon + gap), 3.0),
                NSSize::new(icon, icon),
            )
        })
        .collect();
    Some((selector, icons))
}

fn workspace_accent(workspace_id: &str) -> (f64, f64, f64) {
    let hash = workspace_id.bytes().fold(2_166_136_261u32, |value, byte| {
        (value ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    let palette = [
        (0.29, 0.64, 0.96),
        (0.30, 0.78, 0.56),
        (0.96, 0.58, 0.27),
        (0.91, 0.39, 0.49),
        (0.46, 0.72, 0.86),
    ];
    palette[hash as usize % palette.len()]
}

unsafe fn render_session_selector(
    canvas: *mut objc2::runtime::AnyObject,
    bounds: objc2_foundation::NSRect,
    workspaces: &[PipWorkspaceSummary],
    selected_workspace_id: Option<&str>,
) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    let Some((selector_frame, icons)) =
        session_selector_layout(bounds.size.width, bounds.size.height, workspaces.len())
    else {
        return;
    };
    let Some(selector_owner) =
        adopt_alloc_init(visual_effect_view(selector_frame, 14.0, 6, 1, true))
    else {
        return;
    };
    let selector = owned_object_ptr(&selector_owner);
    let selector_layer: *mut AnyObject = msg_send![selector, layer];
    set_layer_background(selector_layer, color(0.10, 0.12, 0.15, 0.30));
    set_layer_border(selector_layer, 0.6, color(1.0, 1.0, 1.0, 0.26));

    let delegate = HANDLES
        .lock()
        .unwrap()
        .as_ref()
        .map(|handles| handles.delegate as *mut AnyObject)
        .unwrap_or(std::ptr::null_mut());
    for (index, (workspace, icon_frame)) in workspaces.iter().zip(icons).enumerate() {
        let selected = selected_workspace_id == Some(workspace.workspace_id.as_str());
        let (red, green, blue) = workspace_accent(&workspace.workspace_id);
        let Some(button_owner) = adopt_alloc_init({
            let allocated: *mut AnyObject = msg_send![class!(NSButton), alloc];
            msg_send![allocated, initWithFrame: icon_frame]
        }) else {
            continue;
        };
        let button = owned_object_ptr(&button_owner);
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

fn expanded_window_frame(
    current: objc2_foundation::NSRect,
    available: objc2_foundation::NSRect,
) -> objc2_foundation::NSRect {
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let horizontal_margin = 24.0;
    let vertical_margin = 24.0;
    let max_width = (available.size.width - 2.0 * horizontal_margin).max(current.size.width);
    let max_height = (available.size.height - 2.0 * vertical_margin).max(current.size.height);
    let width = (available.size.width * 0.76)
        .max(current.size.width + 180.0)
        .min(max_width);
    let height = (available.size.height * 0.78)
        .max(current.size.height + 140.0)
        .min(max_height);

    NSRect::new(
        NSPoint::new(
            available.origin.x + (available.size.width - width) / 2.0,
            available.origin.y + (available.size.height - height) / 2.0,
        ),
        NSSize::new(width, height),
    )
}

fn rect_from_components(components: [f64; 4]) -> objc2_foundation::NSRect {
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    NSRect::new(
        NSPoint::new(components[0], components[1]),
        NSSize::new(components[2], components[3]),
    )
}

fn rect_components(rect: objc2_foundation::NSRect) -> [f64; 4] {
    [
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    ]
}

unsafe fn update_expand_button(button: *mut objc2::runtime::AnyObject, expanded: bool) {
    use objc2::msg_send;

    if button.is_null() {
        return;
    }
    let title = ns_string(if expanded { "Shrink" } else { "Expand" });
    let tooltip = ns_string(if expanded {
        "Restore compact Agent View"
    } else {
        "Expand Agent View"
    });
    let _: () = msg_send![button, setTitle: title];
    let _: () = msg_send![button, setToolTip: tooltip];
    let _: () = msg_send![button, setAccessibilityLabel: tooltip];
}

fn schedule_auto_collapse(generation: u64) {
    let context = Box::into_raw(Box::new(generation)) as *mut c_void;
    unsafe {
        let deadline = dispatch_time(0, (GLANCE_DURATION_SECONDS * NANOSECONDS_PER_SECOND) as i64);
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_after_f(deadline, main_queue, context, auto_collapse_cb);
    }
}

unsafe extern "C" fn auto_collapse_cb(ctx: *mut c_void) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    let generation = *Box::from_raw(ctx as *mut u64);
    let Some(compact_frame) = EXPANSION_STATE
        .lock()
        .unwrap()
        .end_glance_for_timer(generation)
    else {
        return;
    };
    let (window, button) = {
        let handles = HANDLES.lock().unwrap();
        let Some(handles) = handles.as_ref() else {
            return;
        };
        (
            handles.window as *mut AnyObject,
            handles.expand_button as *mut AnyObject,
        )
    };
    let _: () = msg_send![window, setFrame: rect_from_components(compact_frame) display: true animate: true];
    update_expand_button(button, false);
}

extern "C" fn on_toggle_expanded(
    _delegate: *mut objc2::runtime::AnyObject,
    _selector: objc2::runtime::Sel,
    sender: *mut objc2::runtime::AnyObject,
) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    if sender.is_null() {
        return;
    }
    unsafe {
        let window: *mut AnyObject = msg_send![sender, window];
        if window.is_null() {
            return;
        }
        let current: objc2_foundation::NSRect = msg_send![window, frame];
        let (target, expanded, timer_generation) = {
            let mut state = EXPANSION_STATE.lock().unwrap();
            if state.expanded {
                let target = state
                    .end_glance()
                    .map(rect_from_components)
                    .unwrap_or(current);
                (target, false, None)
            } else {
                let screen: *mut AnyObject = msg_send![window, screen];
                if screen.is_null() {
                    return;
                }
                let available: objc2_foundation::NSRect = msg_send![screen, visibleFrame];
                let generation = state.begin_glance(rect_components(current));
                (
                    expanded_window_frame(current, available),
                    true,
                    Some(generation),
                )
            }
        };
        let _: () = msg_send![window, setFrame: target display: true animate: true];
        update_expand_button(sender, expanded);
        if let Some(generation) = timer_generation {
            schedule_auto_collapse(generation);
        }
    }
}

unsafe fn install_expand_button(
    content_view: *mut objc2::runtime::AnyObject,
    bounds: objc2_foundation::NSRect,
    delegate: *mut objc2::runtime::AnyObject,
) -> *mut objc2::runtime::AnyObject {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let size = NSSize::new(58.0, 26.0);
    let button: *mut AnyObject = {
        let allocated: *mut AnyObject = msg_send![objc2::class!(NSButton), alloc];
        msg_send![
            allocated,
            initWithFrame: NSRect::new(
                NSPoint::new(bounds.size.width - size.width - 12.0, bounds.size.height - size.height - 12.0),
                size,
            )
        ]
    };
    let title = ns_string("Expand");
    let tooltip = ns_string("Expand Agent View");
    let _: () = msg_send![button, setTitle: title];
    let _: () = msg_send![button, setToolTip: tooltip];
    let _: () = msg_send![button, setAccessibilityLabel: tooltip];
    let _: () = msg_send![button, setBordered: false];
    let _: () = msg_send![button, setRefusesFirstResponder: true];
    let _: () = msg_send![button, setAutoresizingMask: 9u64];
    let _: () = msg_send![button, setWantsLayer: true];
    let foreground: *mut AnyObject = msg_send![objc2::class!(NSColor), whiteColor];
    let _: () = msg_send![button, setContentTintColor: foreground];
    let layer: *mut AnyObject = msg_send![button, layer];
    let _: () = msg_send![layer, setCornerRadius: 9.0_f64];
    set_continuous_corners(layer);
    set_layer_background(layer, color(0.04, 0.05, 0.07, 0.82));
    set_layer_border(layer, 0.7, color(1.0, 1.0, 1.0, 0.42));
    let font: *mut AnyObject = msg_send![objc2::class!(NSFont), boldSystemFontOfSize: 10.0_f64];
    let _: () = msg_send![button, setFont: font];
    let _: () = msg_send![button, setTarget: delegate];
    let _: () = msg_send![button, setAction: objc2::sel!(toggleExpanded:)];
    let _: () = msg_send![content_view, addSubview: button];
    button
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
    // Main-queue callbacks are not guaranteed to provide a Rust-visible pool.
    // Drain transient NSString/NSColor/NSData/NSArray values once per frame;
    // the installed view tree keeps the objects it needs through AppKit's
    // normal strong properties.
    objc2::rc::autoreleasepool(|_| unsafe { render_snapshot_in_pool(snapshot) });
}

unsafe fn render_snapshot_in_pool(snapshot: &ViewSnapshot) {
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
            render_target_window(canvas, frame, target_layout, content_height);
        }
        render_dock(canvas, &snapshot.frames, &layout, content_height);
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
    EXPANSION_STATE.lock().unwrap().reset();
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
            builder.add_method(
                objc2::sel!(toggleExpanded:),
                on_toggle_expanded as extern "C" fn(_, _, _),
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
        // `sharedApplication` aborts the process instead of returning an error
        // when this security session cannot reach WindowServer. Probe first so
        // startup remains a normal, reportable failure in SSH/headless hosts.
        if !crate::session::has_graphic_access() {
            anyhow::bail!("Agent View requires macOS graphic-session access");
        }
        let _mtm = objc2_foundation::MainThreadMarker::new().ok_or_else(|| {
            anyhow::anyhow!("Agent View must be initialized on the AppKit main thread")
        })?;
        unsafe { initialize_agent_view(cfg)? };
        Ok(Box::new(MacosPipBackend {
            background_only: cfg.background_only,
        }))
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

unsafe fn discard_unpublished_window(window: *mut objc2::runtime::AnyObject) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    if window.is_null() {
        return;
    }
    let _: () = msg_send![window, setDelegate: std::ptr::null_mut::<AnyObject>()];
    let _: () = msg_send![window, orderOut: std::ptr::null_mut::<AnyObject>()];
    let _: () = msg_send![window, close];
}

unsafe fn initialize_agent_view(cfg: &PipConfig) -> anyhow::Result<()> {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    if HANDLES.lock().unwrap().is_some() {
        anyhow::bail!("Agent View is already running");
    }

    // Establish accessory policy before creating or ordering any AppKit
    // window. Combined with LSUIElement and NSNonactivatingPanel, this keeps
    // startup nonactivating by construction instead of activating first and
    // attempting to repair the user's focus with `deactivate` afterward.
    let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
    if app.is_null() {
        anyhow::bail!("Agent View could not initialize NSApplication");
    }
    let accessory_policy_applied: bool = msg_send![
        app,
        setActivationPolicy: NS_APPLICATION_ACTIVATION_POLICY_ACCESSORY
    ];
    // `setActivationPolicy:` may return false when an LSUIElement process is
    // already running with accessory policy. The observable postcondition is
    // authoritative: accept only when AppKit reports that policy as effective.
    let effective_policy: isize = msg_send![app, activationPolicy];
    validate_accessory_policy_postcondition(accessory_policy_applied, effective_policy)?;
    // Complete NSApplication startup before querying screens or creating the
    // panel. The previous async path got this ordering from the already-running
    // AppKit loop; synchronous startup must establish it explicitly so the
    // visibility postcondition below is meaningful.
    let _: () = msg_send![app, finishLaunching];

    let screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
    if screen.is_null() {
        anyhow::bail!("Agent View cannot start because macOS has no main screen");
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
        anyhow::bail!("Agent View could not create its NSPanel");
    }

    let window_title = if cfg.background_only {
        "Cua Agent View — Background Only"
    } else {
        cfg.title.as_str()
    };
    let window_title = ns_string(window_title);
    if !window_title.is_null() {
        let _: () = msg_send![window, setTitle: window_title];
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
    if content_view.is_null() {
        discard_unpublished_window(window);
        anyhow::bail!("Agent View NSPanel did not provide a content view");
    }
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
    if [
        shell_wallpaper_container,
        shell_wallpaper_view,
        wallpaper_container,
        wallpaper_view,
    ]
    .iter()
    .any(|handle| handle.is_null())
    {
        discard_unpublished_window(window);
        anyhow::bail!("Agent View could not create its wallpaper views");
    }
    let inner_frame = desktop_frame(bounds);

    let canvas: *mut AnyObject = {
        let allocated: *mut AnyObject = msg_send![class!(NSView), alloc];
        msg_send![allocated, initWithFrame: inner_frame]
    };
    if canvas.is_null() {
        discard_unpublished_window(window);
        anyhow::bail!("Agent View could not create its canvas");
    }
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
    if delegate.is_null() {
        discard_unpublished_window(window);
        anyhow::bail!("Agent View could not create its window delegate");
    }
    let _: () = msg_send![window, setDelegate: delegate];
    let expand_button = install_expand_button(content_view, bounds, delegate);
    if expand_button.is_null() {
        discard_unpublished_window(window);
        anyhow::bail!("Agent View could not create its Expand control");
    }
    EXPANSION_STATE.lock().unwrap().reset();
    *HANDLES.lock().unwrap() = Some(NativeHandles {
        window: window as usize,
        canvas_view: canvas as usize,
        shell_wallpaper_container: shell_wallpaper_container as usize,
        shell_wallpaper_view: shell_wallpaper_view as usize,
        wallpaper_container: wallpaper_container as usize,
        wallpaper_view: wallpaper_view as usize,
        delegate: delegate as usize,
        expand_button: expand_button as usize,
    });
    *VIEW_MODEL.lock().unwrap() = Some(PipViewModel::new(12));
    update_wallpaper_frame();
    render_snapshot(&ViewSnapshot {
        frames: Vec::new(),
        workspaces: Vec::new(),
        selected_workspace_id: None,
    });
    let _: () = msg_send![content_view, displayIfNeeded];
    let _: () = msg_send![window, orderFrontRegardless];
    let window_number: isize = msg_send![window, windowNumber];
    let visible: bool = msg_send![window, isVisible];
    let window_server_on_screen = u32::try_from(window_number)
        .ok()
        .filter(|window_id| *window_id > 0)
        .is_some_and(window_server_reports_on_screen);
    if let Err(error) =
        validate_window_server_readiness(window_number, visible, window_server_on_screen)
    {
        // The backend is not usable unless AppKit and WindowServer both agree
        // that the panel is ordered on-screen. This is a synchronous server
        // round-trip, so startup never waits on a main-queue callback before
        // NSApplication.run() and cannot report queued false-success.
        shutdown_cb(std::ptr::null_mut());
        return Err(error);
    }
    tracing::info!(
        target: "pip",
        "Agent View miniature desktop initialised ({}x{})",
        width,
        height
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_init_adoption_balances_a_bounded_render_stress_loop() {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};

        unsafe {
            for _ in 0..2_048 {
                let allocated: *mut AnyObject = msg_send![class!(NSObject), alloc];
                let object: *mut AnyObject = msg_send![allocated, init];
                assert!(!object.is_null());

                // Keep one independent reference alive so retainCount remains
                // queryable after the adopted alloc/init reference is dropped.
                let _: *mut AnyObject = msg_send![object, retain];
                let before: usize = msg_send![object, retainCount];
                let owner = adopt_alloc_init(object).expect("NSObject init returned nil");
                let during: usize = msg_send![owned_object_ptr(&owner), retainCount];
                assert_eq!(during, before);
                drop(owner);
                let after: usize = msg_send![object, retainCount];
                assert_eq!(after + 1, before);
                let _: () = msg_send![object, release];
            }
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
    fn desktop_frame_matches_the_selected_shell_style() {
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        let outer = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(620.0, 420.0));
        let inner = desktop_frame(outer);

        if SHOW_SHELL_BORDER {
            assert_eq!(inner.origin, NSPoint::new(9.0, 10.0));
            assert_eq!(inner.size, NSSize::new(602.0, 390.0));
            assert_eq!(outer.size.height - inner.origin.y - inner.size.height, 20.0);
        } else {
            assert_eq!(inner, outer);
        }
    }

    #[test]
    fn expanded_frame_grows_and_stays_inside_the_visible_screen() {
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        let compact = NSRect::new(NSPoint::new(1050.0, 470.0), NSSize::new(820.0, 540.0));
        let visible = NSRect::new(NSPoint::new(0.0, 40.0), NSSize::new(1920.0, 1040.0));
        let expanded = expanded_window_frame(compact, visible);

        assert!(expanded.size.width > compact.size.width);
        assert!(expanded.size.height > compact.size.height);
        assert!(expanded.origin.x >= visible.origin.x + 24.0);
        assert!(expanded.origin.y >= visible.origin.y + 24.0);
        assert!(expanded.origin.x + expanded.size.width <= visible.origin.x + visible.size.width);
        assert!(expanded.origin.y + expanded.size.height <= visible.origin.y + visible.size.height);
    }

    #[test]
    fn glance_timeout_collapses_only_the_current_expansion() {
        let compact = [1050.0, 470.0, 820.0, 540.0];
        let mut state = ExpansionState::default();

        let first_generation = state.begin_glance(compact);
        assert_eq!(state.end_glance(), Some(compact));

        let second_generation = state.begin_glance(compact);
        assert_ne!(first_generation, second_generation);
        assert_eq!(state.end_glance_for_timer(first_generation), None);
        assert!(state.expanded);
        assert_eq!(state.end_glance_for_timer(second_generation), Some(compact));
        assert!(!state.expanded);
    }

    #[test]
    fn resetting_expansion_invalidates_a_queued_timeout() {
        let mut state = ExpansionState::default();
        let generation = state.begin_glance([10.0, 20.0, 640.0, 420.0]);
        state.reset();

        assert_eq!(state.end_glance_for_timer(generation), None);
        assert!(!state.expanded);
        assert_eq!(state.compact_frame, None);
    }

    #[test]
    fn background_only_routes_keep_local_controls_hit_testable() {
        assert!(!should_yield_native_hit_testing(true));
        assert!(should_yield_native_hit_testing(false));
    }

    #[test]
    fn accessory_policy_readback_is_the_startup_postcondition() {
        assert!(validate_accessory_policy_postcondition(false, 1).is_ok());
        assert!(validate_accessory_policy_postcondition(true, 1).is_ok());

        for (set_attempt_succeeded, effective_policy) in [(false, 0), (true, 2)] {
            let error =
                validate_accessory_policy_postcondition(set_attempt_succeeded, effective_policy)
                    .unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains(&format!("returned {set_attempt_succeeded}")),
                "{message}"
            );
            assert!(
                message.contains(&format!("effective policy is {effective_policy}")),
                "{message}"
            );
        }
    }

    #[test]
    fn startup_requires_a_numbered_visible_window_server_surface() {
        assert_eq!(
            validate_window_server_readiness(42, true, true).unwrap(),
            42
        );

        for (window_number, visible, on_screen, expected) in [
            (0, true, true, "no valid WindowServer number"),
            (42, false, true, "not visible in AppKit"),
            (
                42,
                true,
                false,
                "not present in the on-screen WindowServer list",
            ),
        ] {
            let error =
                validate_window_server_readiness(window_number, visible, on_screen).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn glance_duration_matches_the_eight_second_product_contract() {
        assert_eq!(GLANCE_DURATION_SECONDS, 8);
    }

    #[test]
    fn session_selector_is_local_compact_and_hidden_for_one_session() {
        assert!(session_selector_layout(602.0, 390.0, 1).is_none());
        let (selector, icons) = session_selector_layout(602.0, 390.0, 3).unwrap();
        assert_eq!(icons.len(), 3);
        assert!(selector.origin.x > 0.0);
        assert!(selector.origin.y + selector.size.height <= 390.0);
        assert!(icons.iter().all(|icon| {
            icon.origin.x >= 0.0
                && icon.origin.y >= 0.0
                && icon.origin.x + icon.size.width <= selector.size.width
                && icon.origin.y + icon.size.height <= selector.size.height
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
