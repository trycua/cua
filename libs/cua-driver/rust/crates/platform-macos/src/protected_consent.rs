//! Capture-excluded, automation-hidden protected consent surface for macOS.

use std::cell::RefCell;
use std::ffi::c_void;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use objc2::runtime::{AnyClass, AnyObject};
use objc2::{class, msg_send};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use overlay_ui::{
    place_near_pointer, render_consent, render_indicator, ConsentInteraction, ConsentVisualState,
    HelperDecision, HelperEvent, HelperRequest, InteractionOutcome, Point, Rect, ACCEPT_RECT,
    CONSENT_SIZE, DECLINE_RECT, INDICATOR_SIZE, STOP_RECT,
};

thread_local! {
    static CONTEXT: RefCell<Option<SurfaceContext>> = const { RefCell::new(None) };
    static OUTCOME: RefCell<Option<HelperEvent>> = const { RefCell::new(None) };
}

struct SurfaceContext {
    mode: SurfaceMode,
    layer: usize,
    scale: f32,
    pointer: Point,
}

enum SurfaceMode {
    Consent {
        card: overlay_ui::ConsentCard,
        interaction: ConsentInteraction,
    },
    Indicator {
        card: overlay_ui::IndicatorCard,
        stop_pressed: bool,
    },
}

pub fn run(request: HelperRequest) -> anyhow::Result<()> {
    MainThreadMarker::new()
        .ok_or_else(|| anyhow::anyhow!("protected UI must run on main thread"))?;
    unsafe { run_appkit(request) }
}

unsafe fn run_appkit(request: HelperRequest) -> anyhow::Result<()> {
    let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
    let _: bool = msg_send![app, setActivationPolicy: 1i64];
    let _: () = msg_send![app, finishLaunching];

    let mouse: NSPoint = msg_send![class!(NSEvent), mouseLocation];
    let screen = screen_for_point(mouse);
    if screen.is_null() {
        anyhow::bail!("no interactive NSScreen is available");
    }
    let visible: NSRect = msg_send![screen, visibleFrame];
    let scale = {
        let value: f64 = msg_send![screen, backingScaleFactor];
        if value > 0.0 {
            value as f32
        } else {
            1.0
        }
    };
    let logical_size = match &request {
        HelperRequest::Consent(_) => CONSENT_SIZE,
        HelperRequest::Indicator(_) => INDICATOR_SIZE,
    };
    let pointer = Point {
        x: mouse.x - visible.origin.x,
        y: visible.origin.y + visible.size.height - mouse.y,
    };
    let top_left = match &request {
        HelperRequest::Consent(_) => place_near_pointer(
            pointer,
            Rect {
                x: 0.0,
                y: 0.0,
                width: visible.size.width,
                height: visible.size.height,
            },
            logical_size,
        ),
        HelperRequest::Indicator(_) => Point {
            x: (visible.size.width - logical_size.width - 18.0).max(18.0),
            y: 18.0,
        },
    };
    let frame = NSRect {
        origin: NSPoint {
            x: visible.origin.x + top_left.x,
            y: visible.origin.y + visible.size.height - top_left.y - logical_size.height,
        },
        size: NSSize {
            width: logical_size.width,
            height: logical_size.height,
        },
    };

    let allocated: *mut AnyObject = msg_send![class!(NSWindow), alloc];
    let window: *mut AnyObject = msg_send![allocated,
        initWithContentRect: frame
        styleMask: 0u64
        backing: 2u64
        defer: false
    ];
    if window.is_null() {
        anyhow::bail!("could not create protected NSWindow");
    }
    let _: () = msg_send![window, setOpaque: false];
    let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
    let _: () = msg_send![window, setBackgroundColor: clear];
    let _: () = msg_send![window, setHasShadow: false];
    let _: () = msg_send![window, setIgnoresMouseEvents: false];
    // NSWindowSharingNone: omit the protected pixels from capture APIs.
    let _: () = msg_send![window, setSharingType: 0u64];
    // NSStatusWindowLevel: above ordinary app and agent overlay windows.
    let _: () = msg_send![window, setLevel: 25i64];
    let _: () = msg_send![window, setCollectionBehavior: (1u64 | (1 << 8) | (1 << 4))];
    let _: () = msg_send![window, setReleasedWhenClosed: false];
    let _: () = msg_send![window, setHidesOnDeactivate: false];
    let _: () = msg_send![window, setAcceptsMouseMovedEvents: true];

    let view: *mut AnyObject = msg_send![protected_view_class(), alloc];
    let bounds = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: frame.size,
    };
    let view: *mut AnyObject = msg_send![view, initWithFrame: bounds];
    // `setAcceptsMouseMovedEvents:` only tells the window not to discard
    // movement events; AppKit still needs a tracking area to route them to a
    // custom NSView. Without this, the post-arm entry required by
    // `ConsentInteraction` can never happen and Allow once stays disabled.
    let tracking_area: *mut AnyObject = msg_send![class!(NSTrackingArea), alloc];
    let tracking_area: *mut AnyObject = msg_send![tracking_area,
        initWithRect: bounds
        options: (0x01u64 | 0x02u64 | 0x80u64 | 0x200u64)
        owner: view
        userInfo: std::ptr::null::<AnyObject>()
    ];
    if tracking_area.is_null() {
        anyhow::bail!("could not create protected consent tracking area");
    }
    let _: () = msg_send![view, addTrackingArea: tracking_area];
    let _: () = msg_send![view, setWantsLayer: true];
    let _: () = msg_send![view, setAccessibilityElement: false];
    let _: () = msg_send![window, setContentView: view];
    let _: () = msg_send![window, makeFirstResponder: view];
    let layer: *mut AnyObject = msg_send![view, layer];
    let _: () = msg_send![layer, setContentsScale: scale as f64];

    let context = match request {
        HelperRequest::Consent(card) => SurfaceContext {
            mode: SurfaceMode::Consent {
                card,
                interaction: ConsentInteraction::new(Instant::now(), ACCEPT_RECT, DECLINE_RECT),
            },
            layer: layer as usize,
            scale,
            pointer: Point { x: -1.0, y: -1.0 },
        },
        HelperRequest::Indicator(card) => SurfaceContext {
            mode: SurfaceMode::Indicator {
                card,
                stop_pressed: false,
            },
            layer: layer as usize,
            scale,
            pointer: Point { x: -1.0, y: -1.0 },
        },
    };
    CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
    redraw();

    let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
    let _: () = msg_send![app, activateIgnoringOtherApps: true];
    if matches!(
        CONTEXT.with(|slot| slot
            .borrow()
            .as_ref()
            .map(|ctx| matches!(ctx.mode, SurfaceMode::Consent { .. }))),
        Some(true)
    ) {
        schedule_expiry_timer();
    }
    crate::protected_consent_event(&HelperEvent::Ready)?;
    let _: i64 = msg_send![app, runModalForWindow: window];
    let _: () = msg_send![window, orderOut: std::ptr::null::<AnyObject>()];

    let outcome = OUTCOME
        .with(|slot| slot.borrow_mut().take())
        .unwrap_or_else(|| fallback_outcome());
    crate::protected_consent_event(&outcome)?;
    Ok(())
}

unsafe fn screen_for_point(point: NSPoint) -> *mut AnyObject {
    let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
    let count: usize = msg_send![screens, count];
    for index in 0..count {
        let screen: *mut AnyObject = msg_send![screens, objectAtIndex: index];
        let frame: NSRect = msg_send![screen, frame];
        if point.x >= frame.origin.x
            && point.x <= frame.origin.x + frame.size.width
            && point.y >= frame.origin.y
            && point.y <= frame.origin.y + frame.size.height
        {
            return screen;
        }
    }
    msg_send![class!(NSScreen), mainScreen]
}

fn protected_view_class() -> &'static AnyClass {
    use objc2::declare::ClassBuilder;
    use std::sync::OnceLock;
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut builder = ClassBuilder::new("CuaProtectedConsentView", class!(NSView))
            .expect("CuaProtectedConsentView already registered");
        unsafe {
            builder.add_method(
                objc2::sel!(acceptsFirstResponder),
                accepts_first_responder as extern "C" fn(_, _) -> objc2::runtime::Bool,
            );
            builder.add_method(
                objc2::sel!(mouseMoved:),
                mouse_moved as extern "C" fn(_, _, _),
            );
            builder.add_method(
                objc2::sel!(mouseEntered:),
                mouse_moved as extern "C" fn(_, _, _),
            );
            builder.add_method(
                objc2::sel!(mouseDown:),
                mouse_down as extern "C" fn(_, _, _),
            );
            builder.add_method(objc2::sel!(mouseUp:), mouse_up as extern "C" fn(_, _, _));
            builder.add_method(objc2::sel!(keyDown:), key_down as extern "C" fn(_, _, _));
            builder.add_method(
                objc2::sel!(expiryTick:),
                expiry_tick as extern "C" fn(_, _, _),
            );
        }
        builder.register()
    })
}

extern "C" fn accepts_first_responder(
    _this: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
) -> objc2::runtime::Bool {
    objc2::runtime::Bool::YES
}

extern "C" fn mouse_moved(_this: *mut AnyObject, _cmd: objc2::runtime::Sel, event: *mut AnyObject) {
    let point = event_point(event);
    CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(context) = slot.as_mut() else { return };
        context.pointer = point;
        if let SurfaceMode::Consent { interaction, .. } = &mut context.mode {
            interaction.pointer_moved(point, Instant::now());
        }
    });
    unsafe { redraw() };
}

extern "C" fn mouse_down(_this: *mut AnyObject, _cmd: objc2::runtime::Sel, event: *mut AnyObject) {
    let point = event_point(event);
    CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(context) = slot.as_mut() else { return };
        context.pointer = point;
        match &mut context.mode {
            SurfaceMode::Consent { interaction, .. } => {
                interaction.pointer_down(point, overlay_ui::PointerButton::Primary, Instant::now());
            }
            SurfaceMode::Indicator { stop_pressed, .. } => {
                *stop_pressed = STOP_RECT.contains(point);
            }
        }
    });
}

extern "C" fn mouse_up(_this: *mut AnyObject, _cmd: objc2::runtime::Sel, event: *mut AnyObject) {
    let point = event_point(event);
    let outcome = CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(context) = slot.as_mut() else {
            return None;
        };
        match &mut context.mode {
            SurfaceMode::Consent { card, interaction } => {
                let action = match interaction.pointer_up(point) {
                    InteractionOutcome::Accept => Some(HelperDecision::Accept),
                    InteractionOutcome::Decline => Some(HelperDecision::Decline),
                    _ => None,
                }?;
                Some(HelperEvent::Decision {
                    action,
                    request_digest: card.request_digest.clone(),
                })
            }
            SurfaceMode::Indicator { card, stop_pressed } => {
                let was_pressed = std::mem::take(stop_pressed);
                (was_pressed && STOP_RECT.contains(point)).then(|| HelperEvent::Stop {
                    indicator_id: card.indicator_id.clone(),
                })
            }
        }
    });
    if let Some(outcome) = outcome {
        finish(outcome);
    }
}

extern "C" fn key_down(_this: *mut AnyObject, _cmd: objc2::runtime::Sel, event: *mut AnyObject) {
    let key_code: u16 = unsafe { msg_send![event, keyCode] };
    if key_code == 53 {
        finish(fallback_outcome());
    }
}

extern "C" fn expiry_tick(
    _this: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
    _timer: *mut AnyObject,
) {
    finish(fallback_outcome());
}

fn event_point(event: *mut AnyObject) -> Point {
    let point: NSPoint = unsafe { msg_send![event, locationInWindow] };
    let height = CONTEXT.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|context| match context.mode {
                SurfaceMode::Consent { .. } => CONSENT_SIZE.height,
                SurfaceMode::Indicator { .. } => INDICATOR_SIZE.height,
            })
            .unwrap_or(0.0)
    });
    Point {
        x: point.x,
        y: height - point.y,
    }
}

fn fallback_outcome() -> HelperEvent {
    CONTEXT.with(
        |slot| match slot.borrow().as_ref().map(|context| &context.mode) {
            Some(SurfaceMode::Consent { card, .. }) => HelperEvent::Decision {
                action: HelperDecision::Cancel,
                request_digest: card.request_digest.clone(),
            },
            Some(SurfaceMode::Indicator { card, .. }) => HelperEvent::Stop {
                indicator_id: card.indicator_id.clone(),
            },
            None => HelperEvent::Failed {
                reason: "protected surface closed without context".to_owned(),
            },
        },
    )
}

fn finish(outcome: HelperEvent) {
    OUTCOME.with(|slot| *slot.borrow_mut() = Some(outcome));
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, stopModal];
    }
}

unsafe fn redraw() {
    CONTEXT.with(|slot| {
        let slot = slot.borrow();
        let Some(context) = slot.as_ref() else { return };
        let pixmap = match &context.mode {
            SurfaceMode::Consent { card, interaction } => render_consent(
                card,
                context.scale,
                ConsentVisualState {
                    accept_armed: interaction.accept_armed(Instant::now()),
                    accept_hovered: ACCEPT_RECT.contains(context.pointer),
                    decline_hovered: DECLINE_RECT.contains(context.pointer),
                },
            ),
            SurfaceMode::Indicator { card, .. } => {
                render_indicator(card, context.scale, STOP_RECT.contains(context.pointer))
            }
        };
        let Ok(pixmap) = pixmap else { return };
        let Some(image) = pixmap_to_cgimage(&pixmap) else {
            return;
        };
        let layer = context.layer as *mut AnyObject;
        let _: () = msg_send![layer, setContents: image as *mut AnyObject];
        CGImageRelease(image as *mut c_void);
    });
}

unsafe fn schedule_expiry_timer() {
    let delay = CONTEXT.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|context| match &context.mode {
                SurfaceMode::Consent { card, .. } => Some(card.expires_unix_ms),
                _ => None,
            })
            .map(|expires| {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or(Duration::ZERO)
                    .as_millis();
                u128::from(expires).saturating_sub(now) as f64 / 1000.0
            })
            .unwrap_or(0.1)
            .max(0.1)
    });
    let target: *mut AnyObject = msg_send![protected_view_class(), alloc];
    let target: *mut AnyObject = msg_send![target, init];
    let timer: *mut AnyObject = msg_send![class!(NSTimer),
        timerWithTimeInterval: delay
        target: target
        selector: objc2::sel!(expiryTick:)
        userInfo: std::ptr::null::<AnyObject>()
        repeats: false
    ];
    let run_loop: *mut AnyObject = msg_send![class!(NSRunLoop), currentRunLoop];
    let mode = ns_string("NSModalPanelRunLoopMode");
    let _: () = msg_send![run_loop, addTimer: timer forMode: mode];
}

unsafe fn ns_string(text: &str) -> *mut AnyObject {
    let text = std::ffi::CString::new(text).unwrap_or_default();
    msg_send![class!(NSString), stringWithUTF8String: text.as_ptr() as *const c_void]
}

fn pixmap_to_cgimage(pixmap: &tiny_skia::Pixmap) -> Option<usize> {
    let width = pixmap.width() as usize;
    let height = pixmap.height() as usize;
    if width == 0 || height == 0 {
        return None;
    }
    unsafe extern "C" fn release_data(info: *mut c_void, _data: *const c_void, _size: usize) {
        drop(Box::from_raw(info as *mut Vec<u8>));
    }
    unsafe {
        extern "C" {
            fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
            fn CGColorSpaceRelease(space: *mut c_void);
            fn CGDataProviderCreateWithData(
                info: *mut c_void,
                data: *const c_void,
                size: usize,
                release: Option<unsafe extern "C" fn(*mut c_void, *const c_void, usize)>,
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
        let data = Box::new(pixmap.data().to_vec());
        let ptr = data.as_ptr();
        let len = data.len();
        let info = Box::into_raw(data);
        let color_space = CGColorSpaceCreateDeviceRGB();
        let provider =
            CGDataProviderCreateWithData(info.cast(), ptr.cast(), len, Some(release_data));
        let image = CGImageCreate(
            width,
            height,
            8,
            32,
            width * 4,
            color_space,
            0x4001,
            provider,
            std::ptr::null(),
            false,
            0,
        );
        CGColorSpaceRelease(color_space);
        CGDataProviderRelease(provider);
        (!image.is_null()).then_some(image as usize)
    }
}

extern "C" {
    fn CGImageRelease(image: *mut c_void);
}
