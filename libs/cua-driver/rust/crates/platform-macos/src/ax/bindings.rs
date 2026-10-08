//! Raw FFI bindings to the macOS Accessibility API (AXUIElement).
//!
//! We call the C-level AX API directly rather than using a crate wrapper,
//! because most available crates are incomplete or unmaintained.

#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code
)]

use core_foundation::{
    array::CFArrayRef,
    base::{CFRelease, CFRetain, CFTypeID, CFTypeRef},
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
pub const kAXErrorActionUnsupported: AXError = -25206;
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
    pub fn AXUIElementCopyMultipleAttributeValues(
        element: AXUIElementRef,
        attributes: CFArrayRef,
        options: u32,
        values: *mut CFArrayRef,
    ) -> AXError;
    pub fn AXUIElementCopyAttributeNames(
        element: AXUIElementRef,
        names: *mut CFArrayRef,
    ) -> AXError;
    pub fn AXUIElementCopyActionNames(element: AXUIElementRef, names: *mut CFArrayRef) -> AXError;
    pub fn AXUIElementCopyElementAtPosition(
        application: AXUIElementRef,
        x: f32,
        y: f32,
        element: *mut AXUIElementRef,
    ) -> AXError;
    pub fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
    pub fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementIsAttributeSettable(
        element: AXUIElementRef,
        attribute: CFStringRef,
        settable: *mut u8,
    ) -> AXError;
    pub fn AXUIElementSetMessagingTimeout(
        element: AXUIElementRef,
        timeout_in_seconds: f32,
    ) -> AXError;
    pub fn AXUIElementGetTypeID() -> CFTypeID;
    pub fn AXIsProcessTrusted() -> bool;
    /// `AXIsProcessTrustedWithOptions(options)` — when called with
    /// `{kAXTrustedCheckOptionPrompt: true}` raises the system Accessibility
    /// prompt if the process isn't already trusted.  Returns the post-prompt
    /// trust state (may still be false if the user dismissed the prompt).
    pub fn AXIsProcessTrustedWithOptions(
        options: core_foundation::dictionary::CFDictionaryRef,
    ) -> bool;

    /// Private SPI: maps an AX window element to its CGWindowID.
    /// Stable since macOS 10.9.
    pub fn _AXUIElementGetWindow(element: AXUIElementRef, window_id: *mut u32) -> AXError;

    /// Private SPI: materializes an AX element from its 20-byte remote token
    /// (pid, 0, `'coco'`, element id). Reaches windows on other Spaces, which
    /// `AXWindows` omits. Used by alt-tab-macos for the same purpose.
    pub fn _AXUIElementCreateWithRemoteToken(
        token: core_foundation::data::CFDataRef,
    ) -> AXUIElementRef;
}

/// Hit-test one process's accessibility tree at a screen point. The returned
/// element is retained and must be released by the caller.
///
/// # Safety
///
/// The caller must release any returned element exactly once with `CFRelease`.
pub unsafe fn element_at_screen_position(pid: i32, x: f64, y: f64) -> Option<AXUIElementRef> {
    let application = AXUIElementCreateApplication(pid);
    if application.is_null() {
        return None;
    }
    let mut element = std::ptr::null_mut();
    let error = AXUIElementCopyElementAtPosition(application, x as f32, y as f32, &mut element);
    CFRelease(application as CFTypeRef);
    (error == kAXErrorSuccess && !element.is_null()).then_some(element)
}

// ── AXValue functions ────────────────────────────────────────────────────────
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub fn AXValueGetTypeID() -> CFTypeID;
    pub fn AXValueCreate(the_type: AXValueType, value_ptr: *const c_void) -> AXValueRef;
    pub fn AXValueGetType(value: AXValueRef) -> AXValueType;
    pub fn AXValueGetValue(
        value: AXValueRef,
        the_type: AXValueType,
        value_ptr: *mut c_void,
    ) -> bool;
}

#[repr(C)]
struct CGPointValue {
    x: f64,
    y: f64,
}

#[repr(C)]
struct CGSizeValue {
    width: f64,
    height: f64,
}

// ── Helper functions ──────────────────────────────────────────────────────────

use core_foundation::{array::CFArray, base::TCFType, string::CFString as CFStr};

/// Whether an AX attribute is currently writable on this element.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
pub unsafe fn is_attribute_settable(element: AXUIElementRef, attr_name: &str) -> bool {
    let attr = CFStr::new(attr_name);
    let mut settable = 0_u8;
    AXUIElementIsAttributeSettable(element, attr.as_concrete_TypeRef(), &mut settable)
        == kAXErrorSuccess
        && settable != 0
}

/// Typed conversions used by the tree's batched observation reads.
#[derive(Clone, Copy)]
pub enum AttrKind {
    String,
    Stringish,
    Number,
    Bool,
    Point,
    Size,
}
#[derive(Debug, PartialEq)]
pub enum AttrValue {
    String(String),
    Stringish(StringishAttrValue),
    Number(f64),
    Bool(bool),
    Point([f64; 2]),
    Size([f64; 2]),
}
impl AttrValue {
    pub fn string(self) -> Option<String> {
        if let Self::String(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn stringish(self) -> Option<StringishAttrValue> {
        if let Self::Stringish(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn number(self) -> Option<f64> {
        if let Self::Number(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn boolean(self) -> Option<bool> {
        if let Self::Bool(v) = self {
            Some(v)
        } else {
            None
        }
    }
}
unsafe fn decode_attr(value: CFTypeRef, kind: AttrKind) -> Option<AttrValue> {
    use core_foundation::{boolean::CFBoolean, number::CFNumber};
    if value.is_null() {
        return None;
    }
    let type_id = core_foundation::base::CFGetTypeID(value);
    match kind {
        AttrKind::String if type_id == CFStr::type_id() => Some(AttrValue::String(
            CFStr::wrap_under_get_rule(value as _).to_string(),
        )),
        AttrKind::Stringish => coerce_stringish_value(value).map(AttrValue::Stringish),
        AttrKind::Number if type_id == CFNumber::type_id() => {
            CFNumber::wrap_under_get_rule(value as _)
                .to_f64()
                .map(AttrValue::Number)
        }
        AttrKind::Bool if type_id == CFBoolean::type_id() => Some(AttrValue::Bool(
            CFBoolean::wrap_under_get_rule(value as _).into(),
        )),
        AttrKind::Bool if type_id == CFNumber::type_id() => {
            CFNumber::wrap_under_get_rule(value as _)
                .to_f64()
                .map(|n| AttrValue::Bool(n != 0.0))
        }
        AttrKind::Point
            if type_id == AXValueGetTypeID()
                && AXValueGetType(value as AXValueRef) == kAXValueCGPointType =>
        {
            let mut point = CGPointValue { x: 0.0, y: 0.0 };
            AXValueGetValue(
                value as AXValueRef,
                kAXValueCGPointType,
                &mut point as *mut _ as *mut c_void,
            )
            .then_some(AttrValue::Point([point.x, point.y]))
        }
        AttrKind::Size
            if type_id == AXValueGetTypeID()
                && AXValueGetType(value as AXValueRef) == kAXValueCGSizeType =>
        {
            let mut size = CGSizeValue {
                width: 0.0,
                height: 0.0,
            };
            AXValueGetValue(
                value as AXValueRef,
                kAXValueCGSizeType,
                &mut size as *mut _ as *mut c_void,
            )
            .then_some(AttrValue::Size([size.width, size.height]))
        }
        _ => None,
    }
}
unsafe fn decode_attr_slots(
    array: &CFArray<CFTypeRef>,
    kinds: &[AttrKind],
) -> Vec<Option<AttrValue>> {
    kinds
        .iter()
        .enumerate()
        .map(|(i, kind)| {
            if i >= array.len() as usize {
                return None;
            }
            array
                .get(i as core_foundation::base::CFIndex)
                .and_then(|value| decode_attr(*value, *kind))
        })
        .collect()
}
unsafe fn decode_batch_or_fallback<F>(
    error: AXError,
    values: CFArrayRef,
    kinds: &[AttrKind],
    mut fallback: F,
) -> Vec<Option<AttrValue>>
where
    F: FnMut(usize) -> Option<AttrValue>,
{
    if error != kAXErrorSuccess
        || values.is_null()
        || core_foundation::base::CFGetTypeID(values as CFTypeRef)
            != CFArray::<CFTypeRef>::type_id()
    {
        if !values.is_null() {
            CFRelease(values as CFTypeRef);
        }
        return (0..kinds.len()).map(&mut fallback).collect();
    }
    let array = CFArray::<CFTypeRef>::wrap_under_create_rule(values);
    if array.len() as usize != kinds.len() {
        return (0..kinds.len()).map(&mut fallback).collect();
    }
    decode_attr_slots(&array, kinds)
}
/// Copy heterogeneous AX values, preserving slot positions and existing conversions.
///
/// # Safety
/// `element` must be valid for this call. Each borrowed value is decoded while
/// its owning result array remains retained; no raw value escapes the batch.
/// Whole-API errors use ordinary individual reads with the existing per-object
/// messaging timeout. Slot errors never shift later attributes or abort a batch.
pub unsafe fn copy_typed_attrs(
    element: AXUIElementRef,
    attributes: &[(&str, AttrKind)],
) -> Vec<Option<AttrValue>> {
    if attributes.is_empty() {
        return Vec::new();
    }
    let names: Vec<CFStr> = attributes
        .iter()
        .map(|(name, _)| CFStr::new(name))
        .collect();
    let names_array = CFArray::from_CFTypes(&names);
    let kinds: Vec<AttrKind> = attributes.iter().map(|(_, kind)| *kind).collect();
    let mut values: CFArrayRef = std::ptr::null();
    let error = AXUIElementCopyMultipleAttributeValues(
        element,
        names_array.as_concrete_TypeRef(),
        0,
        &mut values,
    );
    decode_batch_or_fallback(error, values, &kinds, |i| {
        let mut value: CFTypeRef = std::ptr::null();
        let error =
            AXUIElementCopyAttributeValue(element, names[i].as_concrete_TypeRef(), &mut value);
        let result = if error == kAXErrorSuccess {
            decode_attr(value, kinds[i])
        } else {
            None
        };
        if !value.is_null() {
            CFRelease(value);
        }
        result
    })
}
/// Combine typed position/size with the same minimum-size rule as single reads.
pub fn batch_screen_rect(position: Option<AttrValue>, size: Option<AttrValue>) -> Option<[f64; 4]> {
    match (position, size) {
        (Some(AttrValue::Point([x, y])), Some(AttrValue::Size([w, h])))
            if !(w < 1.0 || h < 1.0) =>
        {
            Some([x, y, w, h])
        }
        _ => None,
    }
}

/// Copy a string attribute from an AX element. Returns `None` on any error.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
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

/// Copy the `AXURL` attribute (a `CFURL`) as its absolute URL string.
/// Returns `None` on any error or if the attribute is not a `CFURL`.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
pub unsafe fn copy_url_attr(element: AXUIElementRef) -> Option<String> {
    use core_foundation::url::CFURL;
    let attr = CFStr::new("AXURL");
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != kAXErrorSuccess || value.is_null() {
        return None;
    }
    if core_foundation::base::CFGetTypeID(value) != CFURL::type_id() {
        CFRelease(value);
        return None;
    }
    let url = CFURL::wrap_under_create_rule(value as _);
    Some(url.absolute().get_string().to_string())
}

/// Copy a numeric attribute from an AX element as an `f64`. Returns `None` on
/// any error or if the attribute is not a `CFNumber`. SwiftUI sliders expose a
/// readable numeric `AXValue` even when that value is not settable — this lets
/// the stepping fallback read the control's current position for feedback.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
pub unsafe fn copy_number_attr(element: AXUIElementRef, attr_name: &str) -> Option<f64> {
    use core_foundation::number::CFNumber;
    let attr = CFStr::new(attr_name);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != kAXErrorSuccess || value.is_null() {
        return None;
    }
    let cf_number_type_id = CFNumber::type_id();
    if core_foundation::base::CFGetTypeID(value) != cf_number_type_id {
        CFRelease(value);
        return None;
    }
    let n = CFNumber::wrap_under_create_rule(value as _);
    n.to_f64()
}

/// Copy a boolean attribute from an AX element. Returns `None` on any error
/// or if the attribute is neither a `CFBoolean` nor a `CFNumber` (some apps
/// report AXEnabled/AXSelected as a 0/1 CFNumber instead of a CFBoolean).
///
/// # Safety
///
/// `element` must be a valid Accessibility object reference for the duration
/// of this call.
pub unsafe fn copy_bool_attr(element: AXUIElementRef, attr_name: &str) -> Option<bool> {
    use core_foundation::boolean::CFBoolean;
    use core_foundation::number::CFNumber;
    let attr = CFStr::new(attr_name);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != kAXErrorSuccess || value.is_null() {
        return None;
    }
    let type_id = core_foundation::base::CFGetTypeID(value);
    if type_id == CFBoolean::type_id() {
        let b = CFBoolean::wrap_under_create_rule(value as _);
        return Some(b.into());
    }
    if type_id == CFNumber::type_id() {
        let n = CFNumber::wrap_under_create_rule(value as _);
        return n.to_f64().map(|f| f != 0.0);
    }
    CFRelease(value);
    None
}

unsafe fn coerce_binary_value(value: CFTypeRef) -> Option<bool> {
    use core_foundation::boolean::CFBoolean;
    use core_foundation::number::CFNumber;
    let type_id = core_foundation::base::CFGetTypeID(value);
    if type_id == CFBoolean::type_id() {
        return Some(CFBoolean::wrap_under_get_rule(value as _).into());
    }
    if type_id == CFNumber::type_id() {
        return match CFNumber::wrap_under_get_rule(value as _).to_f64()? {
            0.0 => Some(false),
            1.0 => Some(true),
            _ => None,
        };
    }
    None
}

/// Read a boolean-valued AX attribute (CFBoolean, or a 0/1 CFNumber).
///
/// # Safety
///
/// `element` must be a valid, retained `AXUIElementRef` for the duration of
/// the call.
pub unsafe fn copy_binary_attr(element: AXUIElementRef, attr_name: &str) -> Option<bool> {
    let attr = CFStr::new(attr_name);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != kAXErrorSuccess || value.is_null() {
        return None;
    }
    let result = coerce_binary_value(value);
    CFRelease(value);
    result
}

/// A copied AX attribute represented for both existing string-only consumers
/// and the wider structured control-state response.
#[derive(Debug, PartialEq, Eq)]
pub struct StringishAttrValue {
    /// Present only when the source value was a CFString.
    pub string_value: Option<String>,
    /// CFString as-is, CFNumber as text, or CFBoolean as `"1"` / `"0"`.
    pub state_value: String,
}

/// Convert a borrowed CF value without taking ownership of it.
unsafe fn coerce_stringish_value(value: CFTypeRef) -> Option<StringishAttrValue> {
    use core_foundation::boolean::CFBoolean;
    use core_foundation::number::CFNumber;
    let type_id = core_foundation::base::CFGetTypeID(value);
    if type_id == CFStr::type_id() {
        let string = CFStr::wrap_under_get_rule(value as _).to_string();
        return Some(StringishAttrValue {
            string_value: Some(string.clone()),
            state_value: string,
        });
    }
    if type_id == CFNumber::type_id() {
        let n = CFNumber::wrap_under_get_rule(value as _);
        let f = n.to_f64()?;
        let state_value = if f == f.trunc() && f.abs() < 1e15 {
            format!("{}", f as i64)
        } else {
            format!("{f}")
        };
        return Some(StringishAttrValue {
            string_value: None,
            state_value,
        });
    }
    if type_id == CFBoolean::type_id() {
        let b = CFBoolean::wrap_under_get_rule(value as _);
        return Some(StringishAttrValue {
            string_value: None,
            state_value: if bool::from(b) {
                "1".into()
            } else {
                "0".into()
            },
        });
    }
    None
}

/// Copy an attribute that may be a `CFString`, `CFNumber`, or `CFBoolean`.
///
/// The returned pair lets the tree walker preserve its historical CFString-only
/// markdown while using the same single AX read for structured control state.
/// Numbers render without a trailing `.0` when integral (`8`, not `8.0`), and
/// booleans render as `1`/`0` to match AppKit's two-state controls.
///
/// # Safety
///
/// `element` must be a valid Accessibility object reference for the duration
/// of this call.
pub unsafe fn copy_stringish_attr(
    element: AXUIElementRef,
    attr_name: &str,
) -> Option<StringishAttrValue> {
    let attr = CFStr::new(attr_name);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != kAXErrorSuccess || value.is_null() {
        return None;
    }
    let result = coerce_stringish_value(value);
    CFRelease(value);
    result
}

/// Get the action names for an AX element.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
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
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
pub unsafe fn element_screen_center(element: AXUIElementRef) -> Option<(f64, f64)> {
    // AXPosition → CGPoint
    let pos_attr = CFStr::new("AXPosition");
    let mut pos_ref: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, pos_attr.as_concrete_TypeRef(), &mut pos_ref);
    if err != kAXErrorSuccess || pos_ref.is_null() {
        return None;
    }
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    let mut pos = CGPoint { x: 0.0, y: 0.0 };
    let ok = AXValueGetValue(
        pos_ref as AXValueRef,
        kAXValueCGPointType,
        &mut pos as *mut _ as *mut std::ffi::c_void,
    );
    CFRelease(pos_ref);
    if !ok {
        return None;
    }

    // AXSize → CGSize
    let sz_attr = CFStr::new("AXSize");
    let mut sz_ref: CFTypeRef = std::ptr::null();
    let err2 = AXUIElementCopyAttributeValue(element, sz_attr.as_concrete_TypeRef(), &mut sz_ref);
    if err2 != kAXErrorSuccess || sz_ref.is_null() {
        return None;
    }
    #[repr(C)]
    struct CGSize {
        w: f64,
        h: f64,
    }
    let mut sz = CGSize { w: 0.0, h: 0.0 };
    let ok2 = AXValueGetValue(
        sz_ref as AXValueRef,
        kAXValueCGSizeType,
        &mut sz as *mut _ as *mut std::ffi::c_void,
    );
    CFRelease(sz_ref);
    if !ok2 || sz.w < 1.0 || sz.h < 1.0 {
        return None;
    }

    Some((pos.x + sz.w / 2.0, pos.y + sz.h / 2.0))
}

/// Read the on-screen bounding rect of an AX element.
/// Returns `[x, y, width, height]` in screen coordinates (top-left origin), or `None`.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
pub unsafe fn element_screen_rect(element: AXUIElementRef) -> Option<[f64; 4]> {
    // AXPosition → CGPoint
    let pos_attr = CFStr::new("AXPosition");
    let mut pos_ref: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, pos_attr.as_concrete_TypeRef(), &mut pos_ref);
    if err != kAXErrorSuccess || pos_ref.is_null() {
        return None;
    }
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    let mut pos = CGPoint { x: 0.0, y: 0.0 };
    let ok = AXValueGetValue(
        pos_ref as AXValueRef,
        kAXValueCGPointType,
        &mut pos as *mut _ as *mut std::ffi::c_void,
    );
    CFRelease(pos_ref);
    if !ok {
        return None;
    }

    // AXSize → CGSize
    let sz_attr = CFStr::new("AXSize");
    let mut sz_ref: CFTypeRef = std::ptr::null();
    let err2 = AXUIElementCopyAttributeValue(element, sz_attr.as_concrete_TypeRef(), &mut sz_ref);
    if err2 != kAXErrorSuccess || sz_ref.is_null() {
        return None;
    }
    #[repr(C)]
    struct CGSize {
        w: f64,
        h: f64,
    }
    let mut sz = CGSize { w: 0.0, h: 0.0 };
    let ok2 = AXValueGetValue(
        sz_ref as AXValueRef,
        kAXValueCGSizeType,
        &mut sz as *mut _ as *mut std::ffi::c_void,
    );
    CFRelease(sz_ref);
    if !ok2 || sz.w < 1.0 || sz.h < 1.0 {
        return None;
    }

    Some([pos.x, pos.y, sz.w, sz.h])
}

/// Get the focused UI element of a running application by pid.
/// Returns a retained `AXUIElementRef` that the caller must release, or `None`.
///
/// # Safety
///
/// The caller must release any returned element exactly once with `CFRelease`.
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

/// Return the CGWindowID of the application's focused AX window.
///
/// This is a narrow read-only proof used before global keyboard delivery: an
/// already focused exact window must not be re-activated, because doing so can
/// make a focus-proxy renderer drop its current key target.
pub fn focused_window_id_of_pid(pid: i32) -> Option<u32> {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let window = copy_element_attr(app, "AXFocusedWindow");
        CFRelease(app as CFTypeRef);
        let window = window?;
        let window_id = ax_get_window_id(window);
        CFRelease(window as CFTypeRef);
        window_id
    }
}

/// Get the children of an AX element.
///
/// # Safety
///
/// `element` must be valid, and the caller must release every returned element.
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

/// Copy an AX element-valued attribute. The returned element is retained and
/// must be released by the caller.
///
/// # Safety
///
/// `element` must be valid, and the caller must release any returned element.
pub unsafe fn copy_element_attr(
    element: AXUIElementRef,
    attr_name: &str,
) -> Option<AXUIElementRef> {
    let attr = CFStr::new(attr_name);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != kAXErrorSuccess || value.is_null() {
        return None;
    }
    if core_foundation::base::CFGetTypeID(value) != AXUIElementGetTypeID() {
        CFRelease(value);
        return None;
    }
    Some(value as AXUIElementRef)
}

/// Perform an AX action using a string attribute name.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
pub unsafe fn perform_action(element: AXUIElementRef, action_name: &str) -> AXError {
    let action = CFStr::new(action_name);
    AXUIElementPerformAction(element, action.as_concrete_TypeRef())
}

/// Set an AX attribute to a CFString value.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
pub unsafe fn set_string_attr(element: AXUIElementRef, attr_name: &str, value: &str) -> AXError {
    let attr = CFStr::new(attr_name);
    let cf_value = CFStr::new(value);
    AXUIElementSetAttributeValue(element, attr.as_concrete_TypeRef(), cf_value.as_CFTypeRef())
}

/// Set an AX attribute to a CFNumber (double) value. Numeric controls — most
/// notably `AXSlider` (NSSlider) and `AXStepper` — expose a numeric `AXValue`
/// reject a `CFString` write — `-25200` (kAXErrorFailure, observed live on a
/// SwiftUI `AXSlider`) or `-25201` (kAXErrorIllegalArgument); only a `CFNumber`
/// is accepted. Text fields, by contrast, take a `CFString`.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
pub unsafe fn set_number_attr(element: AXUIElementRef, attr_name: &str, value: f64) -> AXError {
    use core_foundation::number::CFNumber;
    let attr = CFStr::new(attr_name);
    let cf_value = CFNumber::from(value);
    AXUIElementSetAttributeValue(element, attr.as_concrete_TypeRef(), cf_value.as_CFTypeRef())
}

/// Set an AX CGPoint attribute such as `AXPosition`.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
pub unsafe fn set_point_attr(element: AXUIElementRef, attr_name: &str, x: f64, y: f64) -> AXError {
    let attr = CFStr::new(attr_name);
    let point = CGPointValue { x, y };
    let value = AXValueCreate(
        kAXValueCGPointType,
        &point as *const CGPointValue as *const c_void,
    );
    if value.is_null() {
        return kAXErrorFailure;
    }
    let result =
        AXUIElementSetAttributeValue(element, attr.as_concrete_TypeRef(), value as CFTypeRef);
    CFRelease(value as CFTypeRef);
    result
}

/// Set an AX CGSize attribute such as `AXSize`.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
pub unsafe fn set_size_attr(
    element: AXUIElementRef,
    attr_name: &str,
    width: f64,
    height: f64,
) -> AXError {
    let attr = CFStr::new(attr_name);
    let size = CGSizeValue { width, height };
    let value = AXValueCreate(
        kAXValueCGSizeType,
        &size as *const CGSizeValue as *const c_void,
    );
    if value.is_null() {
        return kAXErrorFailure;
    }
    let result =
        AXUIElementSetAttributeValue(element, attr.as_concrete_TypeRef(), value as CFTypeRef);
    CFRelease(value as CFTypeRef);
    result
}

/// Set an AX attribute to a CFBoolean true value.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
pub unsafe fn set_bool_attr_true(element: AXUIElementRef, attr_name: &str) -> AXError {
    use core_foundation::boolean::CFBoolean;
    let attr = CFStr::new(attr_name);
    let cf_true = CFBoolean::true_value();
    AXUIElementSetAttributeValue(element, attr.as_concrete_TypeRef(), cf_true.as_CFTypeRef())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessibilityOptIn {
    ManualAccessibility,
    EnhancedUserInterface,
    NotAccepted,
}

/// Signal to a Chromium/Electron application root that a real assistive client
/// is present so it materializes its full web-content accessibility tree.
///
/// `AXManualAccessibility` is the modern opt-in with no screen-reader side
/// effects; `AXEnhancedUserInterface` is the legacy fallback some Electron
/// builds expose instead (the modern attribute returns
/// `kAXErrorAttributeUnsupported` on those builds).
///
/// # Safety
///
/// `app_element` must be a valid, live application `AXUIElementRef`.
pub unsafe fn enable_chromium_accessibility(app_element: AXUIElementRef) -> AccessibilityOptIn {
    let manual = set_bool_attr_true(app_element, "AXManualAccessibility");
    if manual == kAXErrorSuccess {
        return AccessibilityOptIn::ManualAccessibility;
    }
    if manual != kAXErrorAttributeUnsupported {
        // A transient error (e.g. timeout / app busy) rather than a hard
        // "this app has no such attribute" — don't bother with the legacy
        // fallback, and don't claim enablement happened.
        return AccessibilityOptIn::NotAccepted;
    }
    if set_bool_attr_true(app_element, "AXEnhancedUserInterface") == kAXErrorSuccess {
        AccessibilityOptIn::EnhancedUserInterface
    } else {
        AccessibilityOptIn::NotAccepted
    }
}

/// Get the CGWindowID of an AX window element via the private `_AXUIElementGetWindow` SPI.
/// Returns `None` if the element is not a composited window.
///
/// # Safety
///
/// `element` must be a valid, live window `AXUIElementRef`.
pub unsafe fn ax_get_window_id(element: AXUIElementRef) -> Option<u32> {
    let mut wid: u32 = 0;
    let err = _AXUIElementGetWindow(element, &mut wid);
    if err == kAXErrorSuccess && wid != 0 {
        Some(wid)
    } else {
        None
    }
}

/// Read the `AXWindows` attribute of an application element.
/// Unlike `AXChildren`, this returns the window list regardless of whether
/// the app is frontmost. Returns a Vec of retained AXUIElementRefs.
///
/// # Safety
///
/// `element` must be valid, and the caller must release every returned element.
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

/// CGWindowIDs of `pid`'s `AXWindows`, or `None` when that list cannot be
/// read: the process is not trusted for Accessibility, the app does not
/// answer within a short timeout, or AX reports an error. `None` means
/// "unknown", never "no windows", so callers must not treat it as proof that a
/// window lacks an AX counterpart.
pub fn ax_window_ids_of_pid(pid: i32) -> Option<std::collections::HashSet<u32>> {
    unsafe {
        if !AXIsProcessTrusted() {
            return None;
        }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        AXUIElementSetMessagingTimeout(app, AX_WINDOW_IDS_TIMEOUT_SECONDS);
        let attr = CFStr::new("AXWindows");
        let mut value: CFTypeRef = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut value);
        CFRelease(app as CFTypeRef);
        if err != kAXErrorSuccess || value.is_null() {
            return None;
        }
        if core_foundation::base::CFGetTypeID(value) != CFArray::<CFTypeRef>::type_id() {
            CFRelease(value);
            return None;
        }
        let arr = CFArray::<CFTypeRef>::wrap_under_create_rule(value as _);
        let ax_type_id = AXUIElementGetTypeID();
        Some(
            (0..arr.len())
                .filter_map(|i| {
                    let item = *arr.get(i)?;
                    (core_foundation::base::CFGetTypeID(item) == ax_type_id)
                        .then(|| ax_get_window_id(item as AXUIElementRef))
                        .flatten()
                })
                .collect(),
        )
    }
}

/// AX messaging timeout for [`ax_window_ids_of_pid`], in seconds. Window
/// enumeration calls it once per app, so a hung app must not stall it.
const AX_WINDOW_IDS_TIMEOUT_SECONDS: f32 = 0.25;

/// Highest AX element id probed when looking for an off-Space window.
/// Window elements are allocated early in an app's lifetime (Calculator's
/// main window is id 42); alt-tab-macos probes the same order of magnitude.
const MAX_REMOTE_TOKEN_ELEMENT_ID: u64 = 2_000;

/// Wall-clock ceiling for one remote-token probe, so an unresponsive app
/// cannot stall a snapshot or an action decision.
const REMOTE_TOKEN_PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(300);

/// Per-candidate AX messaging timeout during the probe, in seconds.
const REMOTE_TOKEN_CANDIDATE_TIMEOUT_SECONDS: f32 = 0.05;

/// The 20-byte remote token AX uses to identify one element of `pid`.
fn remote_token_bytes(pid: i32, element_id: u64) -> [u8; 20] {
    const COCOA_TOKEN_MAGIC: i32 = 0x636f_636f; // 'coco'
    let mut token = [0u8; 20];
    token[0..4].copy_from_slice(&pid.to_ne_bytes());
    token[8..12].copy_from_slice(&COCOA_TOKEN_MAGIC.to_ne_bytes());
    token[12..20].copy_from_slice(&element_id.to_ne_bytes());
    token
}

/// Find the `AXWindow` element for `window_id` when `AXWindows` omits it —
/// macOS drops windows on other Spaces from that list. Probes the app's AX
/// element ids through `_AXUIElementCreateWithRemoteToken` and returns only an
/// element whose role is `AXWindow` AND whose `_AXUIElementGetWindow` equals
/// `window_id`, so the result is exactly as strong as an `AXWindows` match.
/// Returns a retained element the caller must release.
///
/// # Safety
///
/// The caller must release any returned element exactly once with `CFRelease`.
pub unsafe fn copy_ax_window_by_remote_token(pid: i32, window_id: u32) -> Option<AXUIElementRef> {
    let started = std::time::Instant::now();
    for element_id in 0..MAX_REMOTE_TOKEN_ELEMENT_ID {
        if started.elapsed() > REMOTE_TOKEN_PROBE_DEADLINE {
            return None;
        }
        let token =
            core_foundation::data::CFData::from_buffer(&remote_token_bytes(pid, element_id));
        let element = _AXUIElementCreateWithRemoteToken(token.as_concrete_TypeRef());
        if element.is_null() {
            continue;
        }
        AXUIElementSetMessagingTimeout(element, REMOTE_TOKEN_CANDIDATE_TIMEOUT_SECONDS);
        if copy_string_attr(element, "AXRole").as_deref() == Some("AXWindow")
            && ax_get_window_id(element) == Some(window_id)
        {
            // The snapshot walk reads the whole subtree through this element;
            // restore the system default so slow apps are not cut off at the
            // probe's per-candidate timeout (issue #4082).
            AXUIElementSetMessagingTimeout(element, 0.0);
            return Some(element);
        }
        CFRelease(element as CFTypeRef);
    }
    None
}

/// Whether to run the remote-token probe for a window `AXWindows` omitted.
///
/// Windows WindowServer reports on another Space qualify. So does a window it
/// places on the current Space but reports off screen: that combination is a
/// stale or mid-transition Space view (issue #4437), in which AX drops the
/// window from `AXWindows` exactly as it does for an off-Space one. An
/// on-screen current-Space or unknown window that AX cannot map fails fast
/// instead of paying the probe's deadline on every call (issue #4083). The
/// WindowServer view is only queried for unlisted windows, so listed windows
/// skip that read.
fn should_probe_off_space_window(
    listed_in_ax_windows: bool,
    space_view: impl FnOnce() -> Option<crate::windows::WindowSpaceView>,
) -> bool {
    if listed_in_ax_windows {
        return false;
    }
    match space_view() {
        Some(view) => match view.on_current_space {
            Some(false) => true,
            Some(true) => !view.is_on_screen,
            None => false,
        },
        None => false,
    }
}

/// `AXWindows` of `pid`'s application element, plus the requested window when
/// `AXWindows` does not list it because it is on another Space. Returns
/// retained elements the caller must release.
///
/// # Safety
///
/// `app` must be the valid application element of `pid`, and the caller must
/// release every returned element.
pub unsafe fn copy_ax_windows_including(
    app: AXUIElementRef,
    pid: i32,
    window_id: u32,
) -> Vec<AXUIElementRef> {
    let mut windows = copy_ax_windows(app);
    let listed = windows
        .iter()
        .any(|&window| ax_get_window_id(window) == Some(window_id));
    if should_probe_off_space_window(listed, || {
        crate::windows::window_space_view_by_id(window_id)
    }) {
        windows.extend(copy_ax_window_by_remote_token(pid, window_id));
    }
    windows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::WindowSpaceView;
    use core_foundation::{boolean::CFBoolean, number::CFNumber};

    #[test]
    fn typed_batch_preserves_error_null_empty_and_later_typed_slots() {
        let empty = CFStr::new("");
        let number = CFNumber::from(8.0);
        let yes = CFBoolean::true_value();
        let error: AXError = kAXErrorAttributeUnsupported;
        let point = CGPointValue { x: 12.5, y: -3.0 };
        let size = CGSizeValue {
            width: 42.0,
            height: 9.5,
        };
        unsafe {
            let err = AXValueCreate(5, &error as *const _ as *const c_void);
            let p = AXValueCreate(kAXValueCGPointType, &point as *const _ as *const c_void);
            let z = AXValueCreate(kAXValueCGSizeType, &size as *const _ as *const c_void);
            let array = CFArray::<CFTypeRef>::from_copyable(&[
                empty.as_CFTypeRef(),
                std::ptr::null(),
                err as CFTypeRef,
                number.as_CFTypeRef(),
                yes.as_CFTypeRef(),
                p as CFTypeRef,
                z as CFTypeRef,
            ]);
            let result = decode_attr_slots(
                &array,
                &[
                    AttrKind::String,
                    AttrKind::String,
                    AttrKind::Number,
                    AttrKind::Stringish,
                    AttrKind::Bool,
                    AttrKind::Point,
                    AttrKind::Size,
                ],
            );
            assert_eq!(
                result,
                vec![
                    Some(AttrValue::String("".into())),
                    None,
                    None,
                    Some(AttrValue::Stringish(StringishAttrValue {
                        string_value: None,
                        state_value: "8".into()
                    })),
                    Some(AttrValue::Bool(true)),
                    Some(AttrValue::Point([12.5, -3.0])),
                    Some(AttrValue::Size([42.0, 9.5]))
                ]
            );
            assert_eq!(decode_attr(p as CFTypeRef, AttrKind::Size), None);
            assert_eq!(decode_attr(z as CFTypeRef, AttrKind::Point), None);
            CFRelease(err as CFTypeRef);
            CFRelease(p as CFTypeRef);
            CFRelease(z as CFTypeRef);
        }
    }
    #[test]
    fn typed_batch_coercions_match_individual_semantics_and_reject_wrong_types() {
        let string = CFStr::new("2.5");
        let number = CFNumber::from(2.5);
        let zero = CFNumber::from(0.0);
        let no = CFBoolean::false_value();
        unsafe {
            assert_eq!(decode_attr(string.as_CFTypeRef(), AttrKind::Number), None);
            assert_eq!(decode_attr(number.as_CFTypeRef(), AttrKind::String), None);
            assert_eq!(decode_attr(no.as_CFTypeRef(), AttrKind::Number), None);
            assert_eq!(
                decode_attr(number.as_CFTypeRef(), AttrKind::Number),
                Some(AttrValue::Number(2.5))
            );
            assert_eq!(
                decode_attr(number.as_CFTypeRef(), AttrKind::Bool),
                Some(AttrValue::Bool(true))
            );
            assert_eq!(
                decode_attr(zero.as_CFTypeRef(), AttrKind::Bool),
                Some(AttrValue::Bool(false))
            );
            assert_eq!(
                decode_attr(no.as_CFTypeRef(), AttrKind::Stringish),
                Some(AttrValue::Stringish(StringishAttrValue {
                    string_value: None,
                    state_value: "0".into()
                }))
            );
            assert_eq!(decode_attr(string.as_CFTypeRef(), AttrKind::Point), None);
        }
    }
    #[test]
    fn typed_batch_whole_error_null_wrong_type_and_short_array_use_exact_fallback() {
        let value = CFStr::new("real");
        let array = CFArray::<CFTypeRef>::from_copyable(&[value.as_CFTypeRef()]);
        for (error, raw) in [
            (kAXErrorFailure, array.as_concrete_TypeRef()),
            (kAXErrorSuccess, std::ptr::null()),
            (kAXErrorSuccess, value.as_CFTypeRef() as CFArrayRef),
            (kAXErrorSuccess, array.as_concrete_TypeRef()),
        ] {
            let mut seen = Vec::new();
            unsafe {
                if !raw.is_null() {
                    CFRetain(raw as CFTypeRef);
                }
                let result = decode_batch_or_fallback(
                    error,
                    raw,
                    &[AttrKind::String, AttrKind::Bool],
                    |i| {
                        seen.push(i);
                        Some(AttrValue::String(format!("single-{i}")))
                    },
                );
                assert_eq!(
                    result,
                    vec![
                        Some(AttrValue::String("single-0".into())),
                        Some(AttrValue::String("single-1".into()))
                    ]
                );
                assert_eq!(seen, vec![0, 1]);
            }
        }
    }
    #[test]
    fn typed_geometry_keeps_fractional_negative_position_and_minimum_size_rule() {
        let rect = |w, h| {
            batch_screen_rect(
                Some(AttrValue::Point([-4.5, 2.25])),
                Some(AttrValue::Size([w, h])),
            )
        };
        assert_eq!(rect(1.0, 1.0), Some([-4.5, 2.25, 1.0, 1.0]));
        for (w, h) in [(0.0, 1.0), (1.0, 0.0), (-1.0, 9.0), (0.999, 2.0)] {
            assert_eq!(rect(w, h), None);
        }
        assert_eq!(
            batch_screen_rect(None, Some(AttrValue::Size([2.0, 2.0]))),
            None
        );
        // Match the former <1 test exactly rather than silently sanitizing NaNs.
        assert!(rect(f64::NAN, 2.0).unwrap()[2].is_nan());
    }

    #[test]
    fn remote_token_layout_is_pid_zero_coco_element_id() {
        let token = remote_token_bytes(0x0102_0304, 42);
        assert_eq!(&token[0..4], &0x0102_0304i32.to_ne_bytes());
        assert_eq!(&token[4..8], &[0, 0, 0, 0]);
        assert_eq!(&token[8..12], &0x636f_636fi32.to_ne_bytes());
        assert_eq!(&token[12..20], &42u64.to_ne_bytes());
    }

    fn space_view(on_current_space: Option<bool>, is_on_screen: bool) -> WindowSpaceView {
        WindowSpaceView {
            on_current_space,
            is_on_screen,
        }
    }

    #[test]
    fn off_space_probe_runs_only_for_unlisted_off_space_windows() {
        assert!(should_probe_off_space_window(false, || Some(space_view(
            Some(false),
            false
        ))));
        assert!(!should_probe_off_space_window(false, || Some(space_view(
            Some(true),
            true
        ))));
        assert!(!should_probe_off_space_window(false, || None));
        assert!(!should_probe_off_space_window(false, || Some(space_view(
            None, false
        ))));
        assert!(!should_probe_off_space_window(true, || {
            panic!("listed windows must not query Space membership")
        }));
    }

    /// Issue #4437: WindowServer placed a visible window on the reported
    /// current Space yet marked it off screen, and AX omitted it. That stale
    /// Space view must re-resolve through the exact-id probe rather than
    /// refuse the window as `ax_unresolved`.
    #[test]
    fn off_space_probe_runs_for_a_current_space_window_reported_off_screen() {
        assert!(should_probe_off_space_window(false, || Some(space_view(
            Some(true),
            false
        ))));
    }

    #[test]
    fn binary_value_accepts_booleans_and_exact_zero_or_one() {
        let true_value = CFBoolean::true_value();
        let false_value = CFBoolean::false_value();
        let zero = CFNumber::from(0.0);
        let one = CFNumber::from(1.0);
        let fractional = CFNumber::from(0.5);
        let other = CFNumber::from(2.0);
        let string = CFStr::new("1");

        assert_eq!(
            unsafe { coerce_binary_value(true_value.as_CFTypeRef()) },
            Some(true)
        );
        assert_eq!(
            unsafe { coerce_binary_value(false_value.as_CFTypeRef()) },
            Some(false)
        );
        assert_eq!(
            unsafe { coerce_binary_value(zero.as_CFTypeRef()) },
            Some(false)
        );
        assert_eq!(
            unsafe { coerce_binary_value(one.as_CFTypeRef()) },
            Some(true)
        );
        assert_eq!(
            unsafe { coerce_binary_value(fractional.as_CFTypeRef()) },
            None
        );
        assert_eq!(unsafe { coerce_binary_value(other.as_CFTypeRef()) }, None);
        assert_eq!(unsafe { coerce_binary_value(string.as_CFTypeRef()) }, None);
    }

    #[test]
    fn binary_value_rejects_near_binary_and_non_finite_numbers() {
        for value in [
            1e-20,
            -1e-20,
            f64::from_bits(1),
            -f64::from_bits(1),
            f64::from_bits(1.0_f64.to_bits() - 1),
            f64::from_bits(1.0_f64.to_bits() + 1),
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            let number = CFNumber::from(value);
            assert_eq!(
                unsafe { coerce_binary_value(number.as_CFTypeRef()) },
                None,
                "unexpected binary state for {value:?}"
            );
        }
    }

    #[test]
    fn stringish_value_coerces_cfstring_cfnumber_and_cfboolean() {
        let string = CFStr::new("Search");
        let integer = CFNumber::from(8.0);
        let decimal = CFNumber::from(2.5);
        let true_value = CFBoolean::true_value();
        let false_value = CFBoolean::false_value();

        let string_result = unsafe { coerce_stringish_value(string.as_CFTypeRef()) }.unwrap();
        assert_eq!(string_result.string_value.as_deref(), Some("Search"));
        assert_eq!(string_result.state_value, "Search");

        let integer_result = unsafe { coerce_stringish_value(integer.as_CFTypeRef()) }.unwrap();
        assert_eq!(integer_result.string_value, None);
        assert_eq!(integer_result.state_value, "8");

        let decimal_result = unsafe { coerce_stringish_value(decimal.as_CFTypeRef()) }.unwrap();
        assert_eq!(decimal_result.string_value, None);
        assert_eq!(decimal_result.state_value, "2.5");

        let true_result = unsafe { coerce_stringish_value(true_value.as_CFTypeRef()) }.unwrap();
        assert_eq!(true_result.string_value, None);
        assert_eq!(true_result.state_value, "1");

        let false_result = unsafe { coerce_stringish_value(false_value.as_CFTypeRef()) }.unwrap();
        assert_eq!(false_result.string_value, None);
        assert_eq!(false_result.state_value, "0");
    }
}
