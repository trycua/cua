// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::ffi::c_void;

use core_foundation::{
    array::CFArray,
    base::{CFRelease, CFTypeRef, TCFType},
    string::{CFString, CFStringRef},
};

use platform_macos::windows::{window_bounds_by_id, WindowBounds};

type AXError = i32;
type AXValueType = i32;
type AXUIElementRef = *mut c_void;
type AXValueRef = *mut c_void;

const AX_ERROR_SUCCESS: AXError = 0;
const AX_VALUE_CG_SIZE: AXValueType = 2;

#[repr(C)]
struct CGSize {
    width: f64,
    height: f64,
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXValueCreate(value_type: AXValueType, value: *const c_void) -> AXValueRef;
    fn _AXUIElementGetWindow(element: AXUIElementRef, window_id: *mut u32) -> AXError;
}

/// Set one top-level window's Accessibility size without changing its
/// position, key-window status, or application activation state.
pub(super) fn resize_window_by_id(
    pid: i32,
    window_id: u32,
    width: f64,
    height: f64,
) -> Result<WindowBounds, String> {
    if !width.is_finite() || !height.is_finite() || width < 1.0 || height < 1.0 {
        return Err("window size must contain positive finite dimensions".into());
    }
    let previous = window_bounds_by_id(window_id);
    unsafe {
        if !AXIsProcessTrusted() {
            return Err("Accessibility permission is required to resize the host window".into());
        }
        let application = AXUIElementCreateApplication(pid);
        if application.is_null() {
            return Err(format!(
                "cannot create Accessibility application for pid {pid}"
            ));
        }
        let windows_attribute = CFString::new("AXWindows");
        let mut windows_value: CFTypeRef = std::ptr::null();
        let copy_error = AXUIElementCopyAttributeValue(
            application,
            windows_attribute.as_concrete_TypeRef(),
            &mut windows_value,
        );
        CFRelease(application as CFTypeRef);
        if copy_error != AX_ERROR_SUCCESS || windows_value.is_null() {
            return Err(format!(
                "cannot read AXWindows for pid {pid}: Accessibility error {copy_error}"
            ));
        }
        let windows = CFArray::<CFTypeRef>::wrap_under_create_rule(windows_value as _);
        let selected = windows.iter().find_map(|candidate| {
            let candidate = *candidate;
            let mut candidate_id = 0_u32;
            (_AXUIElementGetWindow(candidate as AXUIElementRef, &mut candidate_id)
                == AX_ERROR_SUCCESS
                && candidate_id == window_id)
                .then_some(candidate)
        });
        let Some(window) = selected else {
            return Err(format!("window {window_id} is unavailable for pid {pid}"));
        };
        let size = CGSize { width, height };
        let size_value = AXValueCreate(AX_VALUE_CG_SIZE, &size as *const _ as *const c_void);
        if size_value.is_null() {
            return Err("cannot create the Accessibility window size value".into());
        }
        let size_attribute = CFString::new("AXSize");
        let set_error = AXUIElementSetAttributeValue(
            window as AXUIElementRef,
            size_attribute.as_concrete_TypeRef(),
            size_value as CFTypeRef,
        );
        CFRelease(size_value as CFTypeRef);
        if set_error != AX_ERROR_SUCCESS {
            return Err(format!(
                "AXSize write failed with Accessibility error {set_error}"
            ));
        }
    }

    let mut latest = previous.clone();
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        latest = window_bounds_by_id(window_id);
        let settled = latest.as_ref().is_some_and(|bounds| {
            (bounds.width - width).abs() <= 0.5 && (bounds.height - height).abs() <= 0.5
        });
        let clamped = match (&previous, &latest) {
            (Some(before), Some(after)) => {
                (after.width - before.width).abs() > 0.5
                    || (after.height - before.height).abs() > 0.5
            }
            _ => false,
        };
        if settled || clamped {
            break;
        }
    }
    latest.ok_or_else(|| format!("window {window_id} disappeared after the resize"))
}
