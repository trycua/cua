// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! macOS backend for [`super::window`]: CoreGraphics window lists, the
//! dragged window's image, and a listen-only `CGEventTap`.
//!
//! Only reached through the guarded functions in [`super::window`], which
//! refuse under `cfg(test)` or `CUA_ENV_TEST_SANDBOX=1`.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::runloop::CFRunLoop;
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
    MouseEvent, Rect, WindowDragEvent, WindowDragTracker, WindowInfo, WindowSource, encode_png,
    thumbnail_rgba,
};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
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

/// A running event tap.
pub struct Monitor {
    run_loop: Mutex<Option<usize>>,
}

impl Monitor {
    pub fn start(on_event: Box<dyn Fn(WindowDragEvent) + Send + 'static>) -> Result<Self, UxError> {
        let (tx, rx) = std::sync::mpsc::channel::<Result<usize, String>>();
        std::thread::Builder::new()
            .name("cua-window-drag".into())
            .spawn(move || {
                let tracker =
                    Mutex::new(WindowDragTracker::new(OnScreen, std::process::id() as i64));
                let tx2 = tx.clone();
                let result = CGEventTap::with_enabled(
                    CGEventTapLocation::Session,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::ListenOnly,
                    vec![
                        CGEventType::LeftMouseDown,
                        CGEventType::LeftMouseDragged,
                        CGEventType::LeftMouseUp,
                    ],
                    |_proxy, etype, event| {
                        let p = event.location();
                        let m = match etype {
                            CGEventType::LeftMouseDown => Some(MouseEvent::Down),
                            CGEventType::LeftMouseDragged => Some(MouseEvent::Dragged),
                            CGEventType::LeftMouseUp => Some(MouseEvent::Up),
                            _ => None,
                        };
                        if let Some(m) = m {
                            let out = tracker
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .handle(m, p.x, p.y);
                            if let Some(mut ev) = out {
                                // The bundle is looked up once, when the drag starts.
                                if let Some(w) =
                                    ev.window.as_mut().filter(|w| w.bundle_path.is_none())
                                {
                                    w.bundle_path = bundle_path(w.pid, &mut HashMap::new());
                                }
                                on_event(ev);
                            }
                        }
                        CallbackResult::Keep
                    },
                    || {
                        let rl = unsafe { CFRunLoopGetCurrent() } as usize;
                        let _ = tx2.send(Ok(rl));
                        CFRunLoop::run_current();
                    },
                );
                if result.is_err() {
                    let _ = tx.send(Err(
                        "the event tap could not be created (Accessibility or Input \
                         Monitoring withheld)"
                            .into(),
                    ));
                }
            })
            .map_err(|e| UxError::Io(e.to_string()))?;
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(rl)) => Ok(Self {
                run_loop: Mutex::new(Some(rl)),
            }),
            Ok(Err(e)) => Err(UxError::PermissionDenied(e)),
            Err(_) => Err(UxError::Io("the event tap did not start".into())),
        }
    }

    pub fn stop(&self) {
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
