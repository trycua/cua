//! macOS [`PointerShapeBackend`]: what cursor the guest would show at a point.
//!
//! - **Hit-test ("ax")**: the frontmost layer-0 window containing the point
//!   (CGWindowList) names the owning process; `AXUIElementCopyElementAtPosition`
//!   on that application's element finds the element, and
//!   [`crate::pointer_shape_map`] maps its role (walking up to three generic
//!   parents). Window edges come from the same window's frame. The query goes
//!   through the application element rather than the system-wide one so its
//!   50 ms messaging timeout stays per element: setting a timeout on the
//!   system-wide element changes it for every AX call in the process.
//! - **System ("nscursor")**: `NSCursor.currentSystemCursor`, classified by
//!   [`crate::cursor::shape`].
//! - **Probe ("cgwarp")**: a `kCGEventMouseMoved` event posted at the HID tap,
//!   which moves the pointer *and* lets the app under it run its cursor
//!   update (a bare `CGWarpMouseCursorPosition` moves the pointer without
//!   any event, so AppKit cursor rects would not fire). No button is ever
//!   pressed. Then `CGAssociateMouseAndMouseCursorPosition(true)`.
//!
//! Limitations:
//! - Hit-test needs the Accessibility permission for the daemon; without it
//!   every hit-test fails and callers fall back to the arrow.
//! - `currentSystemCursor` reflects the cursor only after the app under the
//!   pointer processed a mouse-moved event and updated it; the probe's dwell
//!   covers this, a bare readout right after a warp may still show the
//!   previous shape.
//! - The window edge band is taken from the window's frame; windows that
//!   cannot be resized (`AXSize` not settable) report no edge.
//! - Custom app cursors (not one of AppKit's standard cursors) come back as
//!   `Custom` from the system readout; the hit-test only knows roles.

use std::ffi::c_void;

use core_foundation::base::{CFRelease, CFTypeRef};
use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use cua_driver_core::pointer_shape::{
    edge_resize_shape, BackendNames, HitTest, PointerShapeBackend, ScreenRect,
};

use crate::ax::bindings::{
    copy_bool_attr, copy_element_attr, copy_number_attr, copy_string_attr, element_screen_rect,
    is_attribute_settable, kAXErrorSuccess, AXIsProcessTrusted, AXUIElementCopyElementAtPosition,
    AXUIElementCreateApplication, AXUIElementRef, AXUIElementSetMessagingTimeout,
};
use crate::pointer_shape_map::{is_generic_role, resolve_ax_chain, AxNode};

/// Per-element AX messaging timeout for the hit-test, in seconds.
const AX_TIMEOUT_S: f32 = 0.05;
/// Edge band in points: inside the frame, outside it, and the corner span.
const EDGE_INSIDE: f64 = 3.0;
const EDGE_OUTSIDE: f64 = 4.0;
const EDGE_CORNER: f64 = 10.0;
/// How far up to look for an enclosing `AXWebArea` from web text.
const WEB_AREA_DEPTH: usize = 16;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreate(source: *const c_void) -> *mut c_void;
    fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
}

/// Owned AX element, released on drop.
struct Element(AXUIElementRef);

impl Drop for Element {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0 as CFTypeRef) };
        }
    }
}

impl Element {
    fn attr(&self, name: &str) -> Option<Element> {
        unsafe { copy_element_attr(self.0, name) }.map(|e| {
            unsafe { AXUIElementSetMessagingTimeout(e, AX_TIMEOUT_S) };
            Element(e)
        })
    }

    fn string(&self, name: &str) -> String {
        unsafe { copy_string_attr(self.0, name) }.unwrap_or_default()
    }

    fn rect(&self) -> Option<ScreenRect> {
        unsafe { element_screen_rect(self.0) }.map(|[x, y, w, h]| ScreenRect::new(x, y, w, h))
    }

    fn node(&self) -> AxNode {
        let role = self.string("AXRole");
        let busy = match role.as_str() {
            "AXBusyIndicator" => true,
            "AXProgressIndicator" => {
                let value = unsafe { copy_number_attr(self.0, "AXValue") };
                let max = unsafe { copy_number_attr(self.0, "AXMaxValue") };
                match (value, max) {
                    (Some(v), Some(m)) => v < m,
                    // Indeterminate progress reports no value.
                    (None, _) => true,
                    _ => false,
                }
            }
            _ => false,
        };
        let editable = role == "AXComboBox" && unsafe { is_attribute_settable(self.0, "AXValue") };
        AxNode {
            subrole: self.string("AXSubrole"),
            enabled: unsafe { copy_bool_attr(self.0, "AXEnabled") },
            editable,
            orientation: if role == "AXSplitter" {
                self.string("AXOrientation")
            } else {
                String::new()
            },
            busy,
            role,
        }
    }
}

/// The macOS backend. Stateless; cheap to construct.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacPointerShapes;

impl MacPointerShapes {
    /// The frontmost normal window whose frame (grown by the edge band)
    /// contains the point: `(pid, frame)`.
    fn window_at(x: f64, y: f64) -> Option<(i32, ScreenRect)> {
        let mut windows = crate::windows::visible_windows();
        windows.sort_by_key(|w| std::cmp::Reverse(w.z_index));
        let own = std::process::id() as i32;
        windows
            .into_iter()
            .filter(|w| w.pid != own && w.layer == 0)
            .map(|w| {
                (
                    w.pid,
                    ScreenRect::new(w.bounds.x, w.bounds.y, w.bounds.width, w.bounds.height),
                )
            })
            .find(|(_, frame)| frame.inflate(EDGE_OUTSIDE).contains(x, y))
    }

    fn hit_test_inner(&self, x: f64, y: f64) -> Option<HitTest> {
        if !unsafe { AXIsProcessTrusted() } {
            return None;
        }
        let Some((pid, frame)) = Self::window_at(x, y) else {
            // Desktop background.
            return Some(HitTest::nothing());
        };
        let app = unsafe { AXUIElementCreateApplication(pid) };
        if app.is_null() {
            return None;
        }
        let app = Element(app);
        unsafe { AXUIElementSetMessagingTimeout(app.0, AX_TIMEOUT_S) };

        let mut raw: AXUIElementRef = std::ptr::null_mut();
        let err = unsafe { AXUIElementCopyElementAtPosition(app.0, x as f32, y as f32, &mut raw) };
        let hit = (err == kAXErrorSuccess && !raw.is_null()).then(|| {
            unsafe { AXUIElementSetMessagingTimeout(raw, AX_TIMEOUT_S) };
            Element(raw)
        });

        // Window edges first: they win over whatever control is under them.
        let resizable = hit
            .as_ref()
            .and_then(|h| h.attr("AXWindow"))
            .map(|w| unsafe { is_attribute_settable(w.0, "AXSize") })
            .unwrap_or(true);
        if resizable {
            if let Some(shape) =
                edge_resize_shape(frame, x, y, EDGE_INSIDE, EDGE_OUTSIDE, EDGE_CORNER)
            {
                return Some(HitTest {
                    shape,
                    role: "edge".into(),
                    window: Some(frame),
                    element: None,
                });
            }
        }
        if !frame.contains(x, y) {
            // In the outside band of a window that cannot be resized.
            return Some(HitTest::nothing());
        }
        let Some(hit) = hit else {
            return Some(HitTest {
                window: Some(frame),
                ..HitTest::nothing()
            });
        };

        // The hit and up to three ancestors, keeping the elements so the
        // deciding one's frame can be reported.
        let mut elements = vec![hit];
        let mut chain = vec![elements[0].node()];
        while elements.len() < 4 && is_generic_role(&chain[chain.len() - 1].role) {
            let Some(parent) = elements[elements.len() - 1].attr("AXParent") else {
                break;
            };
            chain.push(parent.node());
            elements.push(parent);
        }
        let in_web_area = chain[0].role == "AXStaticText" && {
            let mut found = chain.iter().any(|n| n.role == "AXWebArea");
            let mut cursor = elements[elements.len() - 1].attr("AXParent");
            let mut depth = elements.len();
            while !found && depth < WEB_AREA_DEPTH {
                let Some(el) = cursor else { break };
                found = el.string("AXRole") == "AXWebArea";
                cursor = el.attr("AXParent");
                depth += 1;
            }
            found
        };
        let (shape, decider) = resolve_ax_chain(&chain, in_web_area);
        Some(HitTest {
            shape,
            role: decider.map(|i| chain[i].role.clone()).unwrap_or_default(),
            window: Some(frame),
            element: decider.and_then(|i| elements[i].rect()),
        })
    }
}

impl PointerShapeBackend for MacPointerShapes {
    fn names(&self) -> BackendNames {
        BackendNames {
            hit_test: "ax",
            system: if crate::cursor::shape::appkit_cursors_available() {
                "nscursor"
            } else {
                ""
            },
            probe: "cgwarp",
        }
    }

    fn hit_test(&self, x: f64, y: f64) -> Option<HitTest> {
        self.hit_test_inner(x, y)
    }

    fn pointer_position(&self) -> Option<(f64, f64)> {
        unsafe {
            let event = CGEventCreate(std::ptr::null());
            if event.is_null() {
                return None;
            }
            let p = CGEventGetLocation(event);
            CFRelease(event as CFTypeRef);
            Some((p.x, p.y))
        }
    }

    fn warp_pointer(&self, x: f64, y: f64) -> bool {
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
            return false;
        };
        let Ok(event) = CGEvent::new_mouse_event(
            source,
            CGEventType::MouseMoved,
            CGPoint::new(x, y),
            CGMouseButton::Left,
        ) else {
            return false;
        };
        event.post(CGEventTapLocation::HID);
        unsafe { CGAssociateMouseAndMouseCursorPosition(true) };
        true
    }

    fn limitation(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !unsafe { AXIsProcessTrusted() } {
            parts.push("grant Accessibility to cua-spacesd for the hit-test");
        }
        if !crate::cursor::shape::appkit_cursors_available() {
            parts.push("the real cursor is readable only in a process with an NSApplication");
        }
        (!parts.is_empty()).then(|| {
            let mut s = parts.join("; ");
            s.push('.');
            s[..1].make_ascii_uppercase();
            s
        })
    }
}

/// Install the macOS backend (and the system cursor probe it reads through).
pub fn install() -> bool {
    crate::cursor::shape::install();
    cua_driver_core::pointer_shape::set_pointer_shape_backend(MacPointerShapes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live: hit-tests and warps the real pointer. Never run on a user's
    /// machine; only in a disposable macOS guest.
    #[test]
    #[ignore = "moves the real pointer and queries real apps; guest only"]
    fn live_probe_restores_the_pointer() {
        if std::env::var("CUA_ENV_ALLOW_HOST_APP_EFFECTS").as_deref() != Ok("1") {
            return;
        }
        let backend = MacPointerShapes;
        let home = backend.pointer_position().expect("pointer");
        let out = cua_driver_core::pointer_shape::probe_by_warp(
            &backend,
            (home.0 + 40.0, home.1 + 40.0),
            Default::default(),
            &|| true,
        );
        let back = backend.pointer_position().expect("pointer");
        assert!(
            (back.0 - home.0).abs() <= 1.0 && (back.1 - home.1).abs() <= 1.0,
            "{out:?}"
        );
        let _ = backend.hit_test(home.0, home.1);
    }
}
