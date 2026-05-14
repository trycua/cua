//! macOS window enumeration via CGWindowList APIs.
//!
//! Uses the C-level CGWindowListCopyWindowInfo API which returns a CFArray
//! of CFDictionary objects describing each window.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub window_id: u32,
    pub pid: i32,
    pub app_name: String,
    pub title: String,
    pub bounds: WindowBounds,
    pub layer: i32,
    pub z_index: usize,
    pub is_on_screen: bool,
    pub on_current_space: Option<bool>,
    pub space_ids: Option<Vec<u64>>,
}

// ── CGWindow option flags ─────────────────────────────────────────────────────
const kCGWindowListExcludeDesktopElements: u32 = 16;
const kCGWindowListOptionOnScreenOnly: u32 = 1;
const kCGNullWindowID: u32 = 0;

// ── Internal CGWindowInfo parsing ─────────────────────────────────────────────
//
// We use `system_profiler` workaround via `CGWindowListCopyWindowInfo` which
// returns a plist-like structure. The simplest cross-compile-safe approach
// is to dump via `osascript` or use the Objective-C runtime.
//
// For the initial version we use the `core-foundation` crate + direct C linkage.

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(
        option: u32,
        relativeToWindow: u32,
    ) -> core_foundation::array::CFArrayRef;
}

/// Enumerate all windows (including off-screen).
pub fn all_windows() -> Vec<WindowInfo> {
    enumerate_windows(kCGWindowListExcludeDesktopElements)
}

/// Enumerate only on-screen windows.
pub fn visible_windows() -> Vec<WindowInfo> {
    enumerate_windows(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements)
}

fn enumerate_windows(options: u32) -> Vec<WindowInfo> {
    use core_foundation::{
        array::CFArray,
        base::{CFGetTypeID, TCFType, CFTypeRef},
        dictionary::CFDictionary,
        string::CFString,
        number::CFNumber,
        boolean::CFBoolean,
    };
    use std::os::raw::c_void;

    let raw_ref = unsafe {
        CGWindowListCopyWindowInfo(options, kCGNullWindowID)
    };
    if raw_ref.is_null() {
        return vec![];
    }

    let raw: CFArray<CFTypeRef> = unsafe { CFArray::wrap_under_create_rule(raw_ref as _) };
    let total = raw.len() as usize;
    let mut result = Vec::new();

    for (idx, item) in raw.iter().enumerate() {
        let item = *item;
        // Each item should be a CFDictionary.
        let dict_type = CFDictionary::<*const c_void, *const c_void>::type_id();
        if unsafe { CFGetTypeID(item) } != dict_type {
            continue;
        }

        let dict: CFDictionary<*const c_void, *const c_void> = unsafe {
            CFDictionary::wrap_under_get_rule(item as _)
        };

        // Helper: get number from dict by key string.
        let get_num = |key: &str| -> i64 {
            let k = CFString::new(key);
            dict.find(k.as_concrete_TypeRef() as *const c_void)
                .and_then(|v| unsafe {
                    let v = *v;
                    if CFGetTypeID(v) == CFNumber::type_id() {
                        CFNumber::wrap_under_get_rule(v as _).to_i64()
                    } else { None }
                })
                .unwrap_or(0)
        };

        let get_str = |key: &str| -> String {
            let k = CFString::new(key);
            dict.find(k.as_concrete_TypeRef() as *const c_void)
                .and_then(|v| unsafe {
                    let v = *v;
                    if CFGetTypeID(v) == CFString::type_id() {
                        Some(CFString::wrap_under_get_rule(v as _).to_string())
                    } else { None }
                })
                .unwrap_or_default()
        };

        let get_bool = |key: &str| -> bool {
            let k = CFString::new(key);
            dict.find(k.as_concrete_TypeRef() as *const c_void)
                .map(|v| unsafe {
                    let v = *v;
                    if CFGetTypeID(v) == CFBoolean::type_id() {
                        bool::from(CFBoolean::wrap_under_get_rule(v as _))
                    } else { false }
                })
                .unwrap_or(false)
        };

        let window_id = get_num("kCGWindowNumber") as u32;
        let pid = get_num("kCGWindowOwnerPID") as i32;
        let app_name = get_str("kCGWindowOwnerName");
        let title = get_str("kCGWindowName");
        let layer = get_num("kCGWindowLayer") as i32;
        let is_on_screen = get_bool("kCGWindowIsOnscreen");

        // Only include layer-0 windows.
        if layer != 0 { continue; }

        // Parse bounds dict.
        let bounds = {
            let bk = CFString::new("kCGWindowBounds");
            dict.find(bk.as_concrete_TypeRef() as *const c_void)
                .and_then(|v| unsafe {
                    let v = *v;
                    if CFGetTypeID(v) == CFDictionary::<*const c_void, *const c_void>::type_id() {
                        let bd: CFDictionary<*const c_void, *const c_void> =
                            CFDictionary::wrap_under_get_rule(v as _);
                        let x = get_bounds_num(&bd, "X");
                        let y = get_bounds_num(&bd, "Y");
                        let w = get_bounds_num(&bd, "Width");
                        let h = get_bounds_num(&bd, "Height");
                        Some(WindowBounds { x, y, width: w, height: h })
                    } else { None }
                })
                .unwrap_or(WindowBounds { x: 0., y: 0., width: 0., height: 0. })
        };

        // z_index: CGWindowList front-to-back → assign reverse index.
        let z_index = total - idx;

        result.push(WindowInfo {
            window_id,
            pid,
            app_name,
            title,
            bounds,
            layer,
            z_index,
            is_on_screen,
            on_current_space: None,
            space_ids: None,
        });
    }

    result
}

fn get_bounds_num(
    dict: &core_foundation::dictionary::CFDictionary<*const std::os::raw::c_void, *const std::os::raw::c_void>,
    key: &str,
) -> f64 {
    use core_foundation::{
        base::{CFGetTypeID, TCFType},
        number::CFNumber,
        string::CFString,
    };
    use std::os::raw::c_void;

    let k = CFString::new(key);
    dict.find(k.as_concrete_TypeRef() as *const c_void)
        .and_then(|v| unsafe {
            let v = *v;
            if CFGetTypeID(v) == CFNumber::type_id() {
                CFNumber::wrap_under_get_rule(v as _).to_f64()
            } else { None }
        })
        .unwrap_or(0.0)
}

/// Look up a window's bounds by its CGWindowID.
///
/// Returns `None` if the window is not currently known to WindowServer
/// (e.g. it was closed or the window_id is stale).
pub fn window_bounds_by_id(window_id: u32) -> Option<WindowBounds> {
    all_windows()
        .into_iter()
        .find(|w| w.window_id == window_id)
        .map(|w| w.bounds)
}

/// Select the best window_id for a pid.
pub fn resolve_main_window_id(pid: i32) -> anyhow::Result<u32> {
    let windows = all_windows();
    let pid_windows: Vec<&WindowInfo> = windows.iter().filter(|w| w.pid == pid).collect();
    if pid_windows.is_empty() {
        anyhow::bail!("pid {pid} has no windows");
    }
    let mut on_screen: Vec<&&WindowInfo> = pid_windows.iter().filter(|w| w.is_on_screen).collect();
    if !on_screen.is_empty() {
        on_screen.sort_by(|a, b| b.z_index.cmp(&a.z_index));
        return Ok(on_screen[0].window_id);
    }
    let largest = pid_windows.iter().max_by(|a, b| {
        let area_a = a.bounds.width * a.bounds.height;
        let area_b = b.bounds.width * b.bounds.height;
        area_a.partial_cmp(&area_b).unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(largest.unwrap().window_id)
}
