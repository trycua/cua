// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! macOS backend for [`super::window`]: CoreGraphics window lists, the
//! dragged window's image, and a listen-only `CGEventTap`.
//!
//! Only reached through the guarded functions in [`super::window`], which
//! refuse under `cfg(test)` or `CUA_ENV_TEST_SANDBOX=1`.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::number::CFNumber;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult,
};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::window::{
    copy_window_info, create_image, kCGNullWindowID, kCGWindowAlpha, kCGWindowBounds,
    kCGWindowImageBoundsIgnoreFraming, kCGWindowImageNominalResolution, kCGWindowLayer,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionIncludingWindow,
    kCGWindowListOptionOnScreenOnly, kCGWindowName, kCGWindowNumber, kCGWindowOwnerName,
    kCGWindowOwnerPID,
};

use super::UxError;
use super::window::{
    MonitorLife, MouseEvent, Rect, TapDisabled, TapHealth, WindowDragEvent, WindowDragTracker,
    WindowInfo, WindowSource, deliver, encode_png, thumbnail_rgba,
};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopStop(rl: *const c_void);
    fn CFRunLoopGetCurrent() -> *const c_void;
}

// Private but long-stable CoreGraphics Spaces calls (window managers use
// them): whether a window is on any Space, and whether it is ordered in.
unsafe extern "C" {
    fn _CGSDefaultConnection() -> i32;
    fn CGSCopySpacesForWindows(
        cid: i32,
        mask: i32,
        windows: core_foundation::array::CFArrayRef,
    ) -> core_foundation::array::CFArrayRef;
    fn CGSWindowIsOrderedIn(cid: i32, wid: u32, out: *mut u8) -> i32;
}

pub fn ax_trusted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

pub fn request_ax_trust() -> bool {
    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let options = CFDictionary::from_CFType_pairs(&[(
            key.as_CFType(),
            CFBoolean::true_value().as_CFType(),
        )]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) != 0
    }
}

fn num(dict: &CFDictionary<CFString, CFType>, key: CFStringRef) -> Option<CFNumber> {
    let key = unsafe { CFString::wrap_under_get_rule(key) };
    dict.find(&key).and_then(|v| v.downcast::<CFNumber>())
}

fn text(dict: &CFDictionary<CFString, CFType>, key: CFStringRef) -> Option<String> {
    let key = unsafe { CFString::wrap_under_get_rule(key) };
    dict.find(&key)
        .and_then(|v| v.downcast::<CFString>())
        .map(|s| s.to_string())
}

fn bounds(dict: &CFDictionary<CFString, CFType>) -> Option<Rect> {
    let key = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };
    let value = dict.find(&key)?;
    let b: CFDictionary<CFString, CFType> =
        unsafe { TCFType::wrap_under_get_rule(value.as_CFTypeRef() as CFDictionaryRef) };
    let c = |n: &str| {
        b.find(CFString::new(n))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_f64())
    };
    Some(Rect {
        x: c("X")?,
        y: c("Y")?,
        width: c("Width")?,
        height: c("Height")?,
    })
}

fn on_a_space(number: u32) -> bool {
    use core_foundation::array::CFArray;
    unsafe {
        let cid = _CGSDefaultConnection();
        if cid == 0 {
            return true;
        }
        let ids = CFArray::from_CFTypes(&[CFNumber::from(number as i64)]);
        let r = CGSCopySpacesForWindows(cid, 7, ids.as_concrete_TypeRef());
        if r.is_null() {
            return true;
        }
        let spaces: CFArray<CFNumber> = CFArray::wrap_under_create_rule(r);
        !spaces.is_empty()
    }
}

fn ordered_in(number: u32) -> bool {
    unsafe {
        let cid = _CGSDefaultConnection();
        if cid == 0 {
            return true;
        }
        let mut out = 0u8;
        if CGSWindowIsOrderedIn(cid, number, &mut out) != 0 {
            return true;
        }
        out != 0
    }
}

fn bundle_path(pid: i64, cache: &mut HashMap<i64, Option<String>>) -> Option<String> {
    cache
        .entry(pid)
        .or_insert_with(|| {
            use objc2_app_kit::NSRunningApplication;
            let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32)?;
            let url = app.bundleURL()?;
            url.path().map(|p| p.to_string())
        })
        .clone()
}

fn windows_with(options: u32, visibility: bool, with_bundles: bool) -> Vec<WindowInfo> {
    let mut out = vec![];
    let Some(array) = copy_window_info(options, kCGNullWindowID) else {
        return out;
    };
    let mut bundles = HashMap::new();
    for ptr in array.get_all_values() {
        let dict: CFDictionary<CFString, CFType> =
            unsafe { TCFType::wrap_under_get_rule(ptr as CFDictionaryRef) };
        let (number, owner, title, pid, layer, alpha) = unsafe {
            (
                num(&dict, kCGWindowNumber)
                    .and_then(|n| n.to_i64())
                    .unwrap_or(0),
                text(&dict, kCGWindowOwnerName).unwrap_or_default(),
                text(&dict, kCGWindowName).unwrap_or_default(),
                num(&dict, kCGWindowOwnerPID)
                    .and_then(|n| n.to_i64())
                    .unwrap_or(0),
                num(&dict, kCGWindowLayer)
                    .and_then(|n| n.to_i64())
                    .unwrap_or(0) as i32,
                num(&dict, kCGWindowAlpha)
                    .and_then(|n| n.to_f64())
                    .unwrap_or(1.0),
            )
        };
        let window_id = number.max(0) as u32;
        let normal = layer == 0 && !owner.is_empty();
        out.push(WindowInfo {
            window_id,
            pid,
            owner,
            title,
            layer,
            alpha,
            bounds: bounds(&dict),
            visible: !visibility || !normal || (on_a_space(window_id) && ordered_in(window_id)),
            bundle_path: if normal && with_bundles {
                bundle_path(pid, &mut bundles)
            } else {
                None
            },
        });
    }
    out
}

/// Every window across all Spaces (desktop chrome excluded).
pub fn all_windows() -> Vec<WindowInfo> {
    windows_with(kCGWindowListExcludeDesktopElements, true, true)
}

/// On-screen windows of the current Space (for drag hit-testing).
fn on_screen() -> Vec<WindowInfo> {
    windows_with(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        false,
        false,
    )
}

/// The union of the active displays.
pub fn display_union() -> Option<Rect> {
    use core_graphics::display::CGDisplay;
    let ids = CGDisplay::active_displays().ok()?;
    let mut acc: Option<(f64, f64, f64, f64)> = None;
    for id in ids {
        let b = CGDisplay::new(id).bounds();
        let (x0, y0, x1, y1) = (
            b.origin.x,
            b.origin.y,
            b.origin.x + b.size.width,
            b.origin.y + b.size.height,
        );
        acc = Some(match acc {
            None => (x0, y0, x1, y1),
            Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
        });
    }
    acc.map(|(x0, y0, x1, y1)| Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

/// One window's image (`kCGWindowListOptionIncludingWindow`: that window
/// only), downscaled and PNG-encoded in memory. Captured at the display's
/// nominal (1x) resolution: a preview is at most a few hundred pixels wide,
/// and the Retina capture was four times the pixels to copy and drop.
pub fn capture(window_id: u32, max_width: usize) -> Option<Vec<u8>> {
    let null_rect = CGRect::new(
        &CGPoint::new(f64::INFINITY, f64::INFINITY),
        &CGSize::new(0.0, 0.0),
    );
    let image = create_image(
        null_rect,
        kCGWindowListOptionIncludingWindow,
        window_id,
        kCGWindowImageBoundsIgnoreFraming | kCGWindowImageNominalResolution,
    )?;
    if image.bits_per_pixel() != 32 {
        return None;
    }
    let data = image.data();
    let (rgba, w, h) = thumbnail_rgba(
        data.bytes(),
        image.width(),
        image.height(),
        image.bytes_per_row(),
        max_width,
    )?;
    encode_png(&rgba, w, h)
}

struct OnScreen;

impl WindowSource for OnScreen {
    fn windows(&self) -> Vec<WindowInfo> {
        on_screen()
    }
}

/// What the tap's callback does with one event type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TapInput {
    Mouse(MouseEvent),
    /// The system turned the tap off (its callback was too slow, or user
    /// input was disabled): it delivers nothing more until re-enabled.
    Disabled,
    Ignore,
}

fn tap_input(etype: CGEventType) -> TapInput {
    match etype {
        CGEventType::LeftMouseDown => TapInput::Mouse(MouseEvent::Down),
        CGEventType::LeftMouseDragged => TapInput::Mouse(MouseEvent::Dragged),
        CGEventType::LeftMouseUp => TapInput::Mouse(MouseEvent::Up),
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            TapInput::Disabled
        }
        _ => TapInput::Ignore,
    }
}

/// One line in the app's log (stderr): the tap's start, failures and
/// recoveries, which are rare.
fn log(message: &str) {
    eprintln!("[cua] window drag: {message}");
}

/// A running event tap.
///
/// The tap's callback only runs the drag state machine and queues what it
/// emits: a delivery thread hands that to the listener, so a slow listener
/// (the Electron app's waits for its JS thread) never makes macOS disable
/// the tap. When macOS disables it anyway, it is re-enabled; when that
/// keeps happening, or the tap's run loop ends on its own, the monitor ends,
/// the listener gets `stopped`, and [`Self::is_running`] turns `false`.
pub struct Monitor {
    run_loop: Mutex<Option<usize>>,
    life: Arc<MonitorLife>,
}

impl Monitor {
    pub fn start(on_event: Box<dyn Fn(WindowDragEvent) + Send + 'static>) -> Result<Self, UxError> {
        let (tx, rx) = std::sync::mpsc::channel::<Result<usize, String>>();
        let (events, queue) = std::sync::mpsc::channel::<WindowDragEvent>();
        let life = Arc::new(MonitorLife::default());
        std::thread::Builder::new()
            .name("cua-window-drag-events".into())
            .spawn(move || {
                // The bundle is looked up once per drag, off the tap.
                deliver(queue, |mut ev| {
                    if let Some(w) = ev.window.as_mut().filter(|w| w.bundle_path.is_none()) {
                        w.bundle_path = bundle_path(w.pid, &mut HashMap::new());
                    }
                    on_event(ev);
                })
            })
            .map_err(|e| UxError::Io(e.to_string()))?;
        let thread_life = life.clone();
        // `CUA_SPACES_NOTCH_DEBUG=1`: each click's candidate windows too.
        let debug = ["CUA_SPACES_NOTCH_DEBUG", "CUA_WINDOW_DRAG_DEBUG"]
            .iter()
            .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()));
        std::thread::Builder::new()
            .name("cua-window-drag".into())
            .spawn(move || {
                let tracker =
                    Mutex::new(WindowDragTracker::new(OnScreen, std::process::id() as i64));
                let health = Mutex::new(TapHealth::default());
                // The tap's mach port, for re-enabling it from its callback.
                let port = Arc::new(AtomicUsize::new(0));
                let callback_port = port.clone();
                let tap_events = events.clone();
                let tap = CGEventTap::new(
                    CGEventTapLocation::Session,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::ListenOnly,
                    vec![
                        CGEventType::LeftMouseDown,
                        CGEventType::LeftMouseDragged,
                        CGEventType::LeftMouseUp,
                    ],
                    move |_proxy, etype, event| {
                        match tap_input(etype) {
                            TapInput::Mouse(m) => {
                                let p = event.location();
                                let mut t = tracker.lock().unwrap_or_else(|e| e.into_inner());
                                let dragging = t.is_dragging();
                                let out = t.handle(m, p.x, p.y);
                                if debug && m == MouseEvent::Down {
                                    log(&format!(
                                        "mouse down at ({:.0}, {:.0}); windows there: {}",
                                        p.x,
                                        p.y,
                                        t.candidates()
                                    ));
                                }
                                if debug && m == MouseEvent::Up && !dragging {
                                    log("mouse up: no window moved");
                                }
                                drop(t);
                                if let Some(ev) = out {
                                    let _ = tap_events.send(ev);
                                }
                            }
                            TapInput::Disabled => {
                                let verdict = health
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .disabled(Instant::now());
                                let tap = callback_port.load(Ordering::SeqCst) as CFMachPortRef;
                                let back_on = verdict == TapDisabled::Reenable
                                    && !tap.is_null()
                                    && unsafe {
                                        CGEventTapEnable(tap, true);
                                        CGEventTapIsEnabled(tap)
                                    };
                                let why = if matches!(etype, CGEventType::TapDisabledByTimeout) {
                                    "a slow callback"
                                } else {
                                    "user input"
                                };
                                if back_on {
                                    log(&format!("the event tap was disabled ({why}); re-enabled"));
                                } else {
                                    log(&format!(
                                        "the event tap was disabled ({why}) and stays off; stopping"
                                    ));
                                    // Ends the run loop: the monitor reports it died.
                                    unsafe { CFRunLoopStop(CFRunLoopGetCurrent()) };
                                }
                            }
                            TapInput::Ignore => {}
                        }
                        CallbackResult::Keep
                    },
                );
                let Ok(tap) = tap else {
                    log("the event tap could not be created");
                    thread_life.ended();
                    let _ = tx.send(Err(
                        "the event tap could not be created (Accessibility or Input \
                         Monitoring withheld)"
                            .into(),
                    ));
                    return;
                };
                let Ok(source) = tap.mach_port().create_runloop_source(0) else {
                    thread_life.ended();
                    let _ = tx.send(Err("the event tap's run loop source failed".into()));
                    return;
                };
                port.store(
                    tap.mach_port().as_concrete_TypeRef() as usize,
                    Ordering::SeqCst,
                );
                CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
                tap.enable();
                let rl = unsafe { CFRunLoopGetCurrent() } as usize;
                log("the event tap is up");
                let _ = tx.send(Ok(rl));
                CFRunLoop::run_current();
                // Removes the tap (its callback runs no more).
                drop(tap);
                if thread_life.ended() {
                    log("the event tap ended on its own; the app starts a new one");
                    let _ = events.send(WindowDragEvent::stopped());
                }
                // `events` drops here: the delivery thread ends once drained.
            })
            .map_err(|e| UxError::Io(e.to_string()))?;
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(rl)) => Ok(Self {
                run_loop: Mutex::new(Some(rl)),
                life,
            }),
            Ok(Err(e)) => Err(UxError::PermissionDenied(e)),
            Err(_) => Err(UxError::Io("the event tap did not start".into())),
        }
    }

    /// Whether the tap is still up (see the type's docs).
    pub fn is_running(&self) -> bool {
        self.life.is_running()
    }

    pub fn stop(&self) {
        self.life.request_stop();
        if let Some(rl) = self
            .run_loop
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            // CFRunLoopStop is safe to call from any thread.
            unsafe { CFRunLoopStop(rl as *const c_void) };
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// An app's Finder icon (any bundle or file path), PNG in memory, `size`
/// pixels square. Draws only the one representation closest to (not below)
/// that size into a bitmap of exactly that size: encoding the whole icon
/// (`TIFFRepresentation`, every size up to 1024 px) took ~0.4 s per app.
pub fn icon_png(path: &str, size: f64) -> Option<Vec<u8>> {
    use objc2::AllocAnyThread;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext,
        NSImageInterpolation, NSWorkspace,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};
    let px = size.round().clamp(1.0, 1024.0) as isize;
    let ws = NSWorkspace::sharedWorkspace();
    let image = ws.iconForFile(&NSString::from_str(path));
    let reps = image.representations();
    let mut best: Option<(isize, objc2::rc::Retained<objc2_app_kit::NSImageRep>)> = None;
    for i in 0..reps.count() {
        let rep = reps.objectAtIndex(i);
        let w = rep.pixelsWide();
        let better = match &best {
            None => true,
            // The smallest at or above `px`, else the largest.
            Some((b, _)) => (w >= px && (*b < px || w < *b)) || (*b < px && w > *b),
        };
        if better {
            best = Some((w, rep));
        }
    }
    let (_, rep) = best?;
    let canvas = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            px,
            px,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&canvas)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&ctx));
    ctx.setImageInterpolation(NSImageInterpolation::High);
    let drawn = image.drawRepresentation_inRect(
        &rep,
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(px as f64, px as f64)),
    );
    ctx.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    if !drawn {
        return None;
    }
    let props: objc2::rc::Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::new();
    let png =
        unsafe { canvas.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
    Some(png.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_tap_is_told_apart_from_mouse_events() {
        assert_eq!(
            tap_input(CGEventType::TapDisabledByTimeout),
            TapInput::Disabled
        );
        assert_eq!(
            tap_input(CGEventType::TapDisabledByUserInput),
            TapInput::Disabled
        );
        assert_eq!(
            tap_input(CGEventType::LeftMouseDown),
            TapInput::Mouse(MouseEvent::Down)
        );
        assert_eq!(
            tap_input(CGEventType::LeftMouseDragged),
            TapInput::Mouse(MouseEvent::Dragged)
        );
        assert_eq!(
            tap_input(CGEventType::LeftMouseUp),
            TapInput::Mouse(MouseEvent::Up)
        );
        assert_eq!(tap_input(CGEventType::MouseMoved), TapInput::Ignore);
    }
}
