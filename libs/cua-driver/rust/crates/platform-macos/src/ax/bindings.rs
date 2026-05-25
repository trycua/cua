//! Raw FFI bindings to the macOS Accessibility API (AXUIElement).
//!
//! We call the C-level AX API directly rather than using a crate wrapper,
//! because most available crates are incomplete or unmaintained.

#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code)]

use core_foundation::{
    array::CFArrayRef,
    base::{CFIndex, CFRelease, CFRetain, CFTypeID, CFTypeRef},
    string::CFStringRef,
};
use std::os::raw::{c_int, c_void};

// ── AXUIElement opaque type ──────────────────────────────────────────────────

#[repr(C)]
pub struct __AXUIElement(c_void);
pub type AXUIElementRef = *mut __AXUIElement;

// ── AXError ──────────────────────────────────────────────────────────────────

pub type AXError = c_int;
pub const kAXErrorSuccess: AXError = 0;
pub const kAXErrorFailure: AXError = -25200;
pub const kAXErrorInvalidUIElement: AXError = -25202;
pub const kAXErrorAttributeUnsupported: AXError = -25205;
pub const kAXErrorNoValue: AXError = -25212;
pub const kAXErrorAPIDisabled: AXError = -25211;

// ── AXValue opaque type ──────────────────────────────────────────────────────

#[repr(C)]
pub struct __AXValue(c_void);
pub type AXValueRef = *mut __AXValue;

pub type AXValueType = c_int;
pub const kAXValueCGPointType: AXValueType = 1;
pub const kAXValueCGSizeType: AXValueType = 2;
pub const kAXValueCGRectType: AXValueType = 3;
pub const kAXValueCFRangeType: AXValueType = 4;
pub const kAXValueIllegalType: AXValueType = 1_000;

// ── Link to AXUIElement functions ────────────────────────────────────────────
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    pub fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementCopyAttributeNames(
        element: AXUIElementRef,
        names: *mut CFArrayRef,
    ) -> AXError;
    pub fn AXUIElementCopyActionNames(
        element: AXUIElementRef,
        names: *mut CFArrayRef,
    ) -> AXError;
    pub fn AXUIElementPerformAction(
        element: AXUIElementRef,
        action: CFStringRef,
    ) -> AXError;
    pub fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementGetTypeID() -> CFTypeID;
    pub fn AXIsProcessTrusted() -> bool;
    /// `AXIsProcessTrustedWithOptions(options)` — when called with
    /// `{kAXTrustedCheckOptionPrompt: true}` raises the system Accessibility
    /// prompt if the process isn't already trusted.  Returns the post-prompt
    /// trust state (may still be false if the user dismissed the prompt).
    pub fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef) -> bool;

    /// Private SPI: maps an AX window element to its CGWindowID.
    /// Stable since macOS 10.9; used by yabai, Hammerspoon, Accessibility Inspector.
    pub fn _AXUIElementGetWindow(element: AXUIElementRef, window_id: *mut u32) -> AXError;
}

// ── AXValue functions ────────────────────────────────────────────────────────
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub fn AXValueGetType(value: AXValueRef) -> AXValueType;
    pub fn AXValueGetValue(value: AXValueRef, the_type: AXValueType, value_ptr: *mut c_void) -> bool;
}

// ── Helper functions ──────────────────────────────────────────────────────────

use core_foundation::{
    array::CFArray,
    base::TCFType,
    string::CFString as CFStr,
};

/// Copy a string attribute from an AX element. Returns `None` on any error.
pub unsafe fn copy_string_attr(element: AXUIElementRef, attr_name: &str) -> Option<String> {
    let attr = CFStr::new(attr_name);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != kAXErrorSuccess || value.is_null() {
        return None;
    }
    let cf_string_type_id = CFStr::type_id();
    if core_foundation::base::CFGetTypeID(value) != cf_string_type_id {
        CFRelease(value);
        return None;
    }
    let s = CFStr::wrap_under_create_rule(value as _);
    Some(s.to_string())
}

/// Get the action names for an AX element.
pub unsafe fn copy_action_names(element: AXUIElementRef) -> Vec<String> {
    let mut names: CFArrayRef = std::ptr::null_mut();
    let err = AXUIElementCopyActionNames(element, &mut names);
    if err != kAXErrorSuccess || names.is_null() {
        return vec![];
    }
    // Use CFArray<CFStr> (the typed wrapper) to satisfy FromVoid bound.
    let arr = CFArray::<CFStr>::wrap_under_create_rule(names);
    (0..arr.len())
        .filter_map(|i| {
            let cf = arr.get(i)?;
            Some(cf.to_string())
        })
        .collect()
}

/// Read the on-screen center of an AX element (AXPosition + AXSize → center).
/// Returns `(cx, cy)` in screen coordinates, or `None` if either attribute
/// is unavailable or the element has zero size.
pub unsafe fn element_screen_center(element: AXUIElementRef) -> Option<(f64, f64)> {
    // AXPosition → CGPoint
    let pos_attr = CFStr::new("AXPosition");
    let mut pos_ref: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, pos_attr.as_concrete_TypeRef(), &mut pos_ref);
    if err != kAXErrorSuccess || pos_ref.is_null() {
        return None;
    }
    #[repr(C)]
    struct CGPoint { x: f64, y: f64 }
    let mut pos = CGPoint { x: 0.0, y: 0.0 };
    let ok = AXValueGetValue(
        pos_ref as AXValueRef,
        kAXValueCGPointType,
        &mut pos as *mut _ as *mut std::ffi::c_void,
    );
    CFRelease(pos_ref);
    if !ok { return None; }

    // AXSize → CGSize
    let sz_attr = CFStr::new("AXSize");
    let mut sz_ref: CFTypeRef = std::ptr::null();
    let err2 = AXUIElementCopyAttributeValue(element, sz_attr.as_concrete_TypeRef(), &mut sz_ref);
    if err2 != kAXErrorSuccess || sz_ref.is_null() {
        return None;
    }
    #[repr(C)]
    struct CGSize { w: f64, h: f64 }
    let mut sz = CGSize { w: 0.0, h: 0.0 };
    let ok2 = AXValueGetValue(
        sz_ref as AXValueRef,
        kAXValueCGSizeType,
        &mut sz as *mut _ as *mut std::ffi::c_void,
    );
    CFRelease(sz_ref);
    if !ok2 || sz.w < 1.0 || sz.h < 1.0 { return None; }

    Some((pos.x + sz.w / 2.0, pos.y + sz.h / 2.0))
}

/// Read the on-screen bounding rect of an AX element.
/// Returns `[x, y, width, height]` in screen coordinates (top-left origin), or `None`.
pub unsafe fn element_screen_rect(element: AXUIElementRef) -> Option<[f64; 4]> {
    // AXPosition → CGPoint
    let pos_attr = CFStr::new("AXPosition");
    let mut pos_ref: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, pos_attr.as_concrete_TypeRef(), &mut pos_ref);
    if err != kAXErrorSuccess || pos_ref.is_null() {
        return None;
    }
    #[repr(C)]
    struct CGPoint { x: f64, y: f64 }
    let mut pos = CGPoint { x: 0.0, y: 0.0 };
    let ok = AXValueGetValue(
        pos_ref as AXValueRef,
        kAXValueCGPointType,
        &mut pos as *mut _ as *mut std::ffi::c_void,
    );
    CFRelease(pos_ref);
    if !ok { return None; }

    // AXSize → CGSize
    let sz_attr = CFStr::new("AXSize");
    let mut sz_ref: CFTypeRef = std::ptr::null();
    let err2 = AXUIElementCopyAttributeValue(element, sz_attr.as_concrete_TypeRef(), &mut sz_ref);
    if err2 != kAXErrorSuccess || sz_ref.is_null() {
        return None;
    }
    #[repr(C)]
    struct CGSize { w: f64, h: f64 }
    let mut sz = CGSize { w: 0.0, h: 0.0 };
    let ok2 = AXValueGetValue(
        sz_ref as AXValueRef,
        kAXValueCGSizeType,
        &mut sz as *mut _ as *mut std::ffi::c_void,
    );
    CFRelease(sz_ref);
    if !ok2 || sz.w < 1.0 || sz.h < 1.0 { return None; }

    Some([pos.x, pos.y, sz.w, sz.h])
}

/// Get the focused UI element of a running application by pid.
/// Returns a retained `AXUIElementRef` that the caller must release, or `None`.
pub unsafe fn focused_element_of_pid(pid: i32) -> Option<AXUIElementRef> {
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return None;
    }
    let attr = CFStr::new("AXFocusedUIElement");
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut value);
    CFRelease(app as CFTypeRef);
    if err != kAXErrorSuccess || value.is_null() {
        return None;
    }
    let ax_type_id = AXUIElementGetTypeID();
    if core_foundation::base::CFGetTypeID(value) != ax_type_id {
        CFRelease(value);
        return None;
    }
    // Already retained by CopyAttributeValue — hand the raw pointer to the caller.
    Some(value as AXUIElementRef)
}

/// Get the children of an AX element.
pub unsafe fn copy_children(element: AXUIElementRef) -> Vec<AXUIElementRef> {
    let attr = CFStr::new("AXChildren");
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != kAXErrorSuccess || value.is_null() {
        return vec![];
    }
    let cf_array_type_id = CFArray::<CFTypeRef>::type_id();
    if core_foundation::base::CFGetTypeID(value) != cf_array_type_id {
        CFRelease(value);
        return vec![];
    }
    let arr = CFArray::<CFTypeRef>::wrap_under_create_rule(value as _);
    let ax_type_id = AXUIElementGetTypeID();
    (0..arr.len())
        .filter_map(|i| {
            let item = *arr.get(i)?;
            if core_foundation::base::CFGetTypeID(item) == ax_type_id {
                // Retain so we own it — caller is responsible for releasing.
                CFRetain(item);
                Some(item as AXUIElementRef)
            } else {
                None
            }
        })
        .collect()
}

/// Perform an AX action using a string attribute name.
pub unsafe fn perform_action(element: AXUIElementRef, action_name: &str) -> AXError {
    let action = CFStr::new(action_name);
    AXUIElementPerformAction(element, action.as_concrete_TypeRef())
}

/// Set an AX attribute to a CFString value.
pub unsafe fn set_string_attr(element: AXUIElementRef, attr_name: &str, value: &str) -> AXError {
    let attr = CFStr::new(attr_name);
    let cf_value = CFStr::new(value);
    AXUIElementSetAttributeValue(element, attr.as_concrete_TypeRef(), cf_value.as_CFTypeRef())
}

/// Set an AX attribute to a CFBoolean true value.
pub unsafe fn set_bool_attr_true(element: AXUIElementRef, attr_name: &str) -> AXError {
    use core_foundation::boolean::CFBoolean;
    let attr = CFStr::new(attr_name);
    let cf_true = CFBoolean::true_value();
    AXUIElementSetAttributeValue(element, attr.as_concrete_TypeRef(), cf_true.as_CFTypeRef())
}

/// Get the CGWindowID of an AX window element via the private `_AXUIElementGetWindow` SPI.
/// Returns `None` if the element is not a composited window.
pub unsafe fn ax_get_window_id(element: AXUIElementRef) -> Option<u32> {
    let mut wid: u32 = 0;
    let err = _AXUIElementGetWindow(element, &mut wid);
    if err == kAXErrorSuccess && wid != 0 { Some(wid) } else { None }
}

/// Read the `AXWindows` attribute of an application element.
/// Unlike `AXChildren`, this returns the window list regardless of whether
/// the app is frontmost. Returns a Vec of retained AXUIElementRefs.
pub unsafe fn copy_ax_windows(element: AXUIElementRef) -> Vec<AXUIElementRef> {
    let attr = CFStr::new("AXWindows");
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != kAXErrorSuccess || value.is_null() {
        return vec![];
    }
    let cf_array_type_id = CFArray::<CFTypeRef>::type_id();
    if core_foundation::base::CFGetTypeID(value) != cf_array_type_id {
        CFRelease(value);
        return vec![];
    }
    let arr = CFArray::<CFTypeRef>::wrap_under_create_rule(value as _);
    let ax_type_id = AXUIElementGetTypeID();
    (0..arr.len())
        .filter_map(|i| {
            let item = *arr.get(i)?;
            if core_foundation::base::CFGetTypeID(item) == ax_type_id {
                CFRetain(item);
                Some(item as AXUIElementRef)
            } else {
                None
            }
        })
        .collect()
}
