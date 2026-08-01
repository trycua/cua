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
// Apple-canonical kCG* naming preserved to match the public Apple headers — the
// upper-case-globals lint would rename them to KCG_..., which would silently
// shadow the Apple-namespaced constant references in any future code that
// re-introduces them. Mirrors platform-windows::uia/windows_enum.rs which uses
// the same allow for UIA_* constants.
#[allow(non_upper_case_globals)]
const kCGWindowListExcludeDesktopElements: u32 = 16;
#[allow(non_upper_case_globals)]
const kCGWindowListOptionOnScreenOnly: u32 = 1;
#[allow(non_upper_case_globals)]
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
    enumerate_windows(kCGWindowListExcludeDesktopElements, LayerFilter::ZeroOnly)
}

/// Enumerate only on-screen windows.
pub fn visible_windows() -> Vec<WindowInfo> {
    enumerate_windows(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        LayerFilter::ZeroOnly,
    )
}

/// Enumerate windows on every CGWindow layer, including the accessory layers
/// (`layer != 0`) that [`all_windows`] hides.
///
/// Only used to answer "does this CGWindowID exist, and who owns it?" — the
/// question `list_windows` must NOT answer, because surfacing tooltips,
/// popovers, the Dock and every NSMenu window would swamp callers. Keeping the
/// layer filter on enumeration and off identity lookup is what lets
/// `get_window_state` tell "no such window" apart from "exists, but is not a
/// layer-0 window" (issue #2237).
fn all_windows_any_layer() -> Vec<WindowInfo> {
    enumerate_windows(kCGWindowListExcludeDesktopElements, LayerFilter::AnyLayer)
}

/// Which CGWindow layers an enumeration admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayerFilter {
    /// Normal application windows only — what `list_windows` reports.
    ZeroOnly,
    /// Every layer, accessory windows included.
    AnyLayer,
}

fn enumerate_windows(options: u32, layers: LayerFilter) -> Vec<WindowInfo> {
    use core_foundation::{
        array::CFArray,
        base::{CFGetTypeID, CFTypeRef, TCFType},
        boolean::CFBoolean,
        dictionary::CFDictionary,
        number::CFNumber,
        string::CFString,
    };
    use std::os::raw::c_void;

    let raw_ref = unsafe { CGWindowListCopyWindowInfo(options, kCGNullWindowID) };
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

        let dict: CFDictionary<*const c_void, *const c_void> =
            unsafe { CFDictionary::wrap_under_get_rule(item as _) };

        // Helper: get number from dict by key string.
        let get_num = |key: &str| -> i64 {
            let k = CFString::new(key);
            dict.find(k.as_concrete_TypeRef() as *const c_void)
                .and_then(|v| unsafe {
                    let v = *v;
                    if CFGetTypeID(v) == CFNumber::type_id() {
                        CFNumber::wrap_under_get_rule(v as _).to_i64()
                    } else {
                        None
                    }
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
                    } else {
                        None
                    }
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
                    } else {
                        false
                    }
                })
                .unwrap_or(false)
        };

        let window_id = get_num("kCGWindowNumber") as u32;
        let pid = get_num("kCGWindowOwnerPID") as i32;
        let app_name = get_str("kCGWindowOwnerName");
        let title = get_str("kCGWindowName");
        let layer = get_num("kCGWindowLayer") as i32;
        let is_on_screen = get_bool("kCGWindowIsOnscreen");

        // Only include layer-0 windows, unless the caller asked for every layer.
        if layer != 0 && layers == LayerFilter::ZeroOnly {
            continue;
        }

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
                        Some(WindowBounds {
                            x,
                            y,
                            width: w,
                            height: h,
                        })
                    } else {
                        None
                    }
                })
                .unwrap_or(WindowBounds {
                    x: 0.,
                    y: 0.,
                    width: 0.,
                    height: 0.,
                })
        };

        // z_index: CGWindowList front-to-back → assign reverse index.
        let z_index = z_index_from_front_to_back(total, idx);

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

fn z_index_from_front_to_back(total: usize, position: usize) -> usize {
    total.saturating_sub(position)
}

fn get_bounds_num(
    dict: &core_foundation::dictionary::CFDictionary<
        *const std::os::raw::c_void,
        *const std::os::raw::c_void,
    >,
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
            } else {
                None
            }
        })
        .unwrap_or(0.0)
}

/// Look up a window by its CGWindowID across every layer.
///
/// Returns `None` only when WindowServer has no record of the id at all —
/// which is precisely the "closed or fabricated window_id" signal callers need.
pub fn window_info_by_id(window_id: u32) -> Option<WindowInfo> {
    all_windows_any_layer()
        .into_iter()
        .find(|w| w.window_id == window_id)
}

/// Look up a window's bounds by its CGWindowID.
///
/// Returns `None` if the window is not currently known to WindowServer
/// (e.g. it was closed or the window_id is stale).
pub fn window_bounds_by_id(window_id: u32) -> Option<WindowBounds> {
    window_info_by_id(window_id).map(|w| w.bounds)
}

/// Who owns a requested CGWindowID, as seen by a caller that asked about `pid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowOwner {
    /// The window exists and `pid` owns it.
    SamePid,
    /// The window exists, but a different process owns it. macOS hosts a
    /// sandboxed app's Open/Save panel in
    /// `com.apple.appkit.xpc.openAndSavePanelService`, so the panel's
    /// CGWindowID belongs to that service and not to the app that opened it
    /// (issue #2237).
    ForeignPid {
        owner_pid: i32,
        owner_app_name: String,
    },
    /// WindowServer has no record of the id — closed, stale, or fabricated.
    Unknown,
}

/// Pure form of [`resolve_window_owner`] over an already-enumerated window
/// list, so the ownership decision is testable without a WindowServer.
pub fn resolve_window_owner_in(windows: &[WindowInfo], pid: i32, window_id: u32) -> WindowOwner {
    match windows.iter().find(|w| w.window_id == window_id) {
        None => WindowOwner::Unknown,
        Some(w) if w.pid == pid => WindowOwner::SamePid,
        Some(w) => WindowOwner::ForeignPid {
            owner_pid: w.pid,
            owner_app_name: w.app_name.clone(),
        },
    }
}

/// Resolve whether `pid` really owns `window_id`. Blocking (one CGWindowList
/// enumeration).
pub fn resolve_window_owner(pid: i32, window_id: u32) -> WindowOwner {
    resolve_window_owner_in(&all_windows_any_layer(), pid, window_id)
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
        on_screen.sort_by_key(|window| std::cmp::Reverse(window.z_index));
        return Ok(on_screen[0].window_id);
    }
    let largest = pid_windows.iter().max_by(|a, b| {
        let area_a = a.bounds.width * a.bounds.height;
        let area_b = b.bounds.width * b.bounds.height;
        area_a
            .partial_cmp(&area_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(largest.unwrap().window_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cg_front_to_back_order_normalizes_to_higher_is_frontmost() {
        let indices: Vec<_> = (0..3)
            .map(|position| z_index_from_front_to_back(3, position))
            .collect();
        assert_eq!(indices, vec![3, 2, 1]);
        assert!(indices[0] > indices[2]);
    }

    fn window(window_id: u32, pid: i32, app_name: &str) -> WindowInfo {
        WindowInfo {
            window_id,
            pid,
            app_name: app_name.into(),
            title: String::new(),
            bounds: WindowBounds {
                x: 0.,
                y: 580.,
                width: 500.,
                height: 500.,
            },
            layer: 0,
            z_index: 1,
            is_on_screen: true,
            on_current_space: None,
            space_ids: None,
        }
    }

    #[test]
    fn owner_resolves_same_pid() {
        let windows = vec![window(42, 800, "TextEdit")];
        assert_eq!(
            resolve_window_owner_in(&windows, 800, 42),
            WindowOwner::SamePid
        );
    }

    /// Issue #2237: TextEdit's Open panel is a layer-0 CGWindow owned by
    /// `com.apple.appkit.xpc.openAndSavePanelService`, not by TextEdit. The
    /// caller must be told the real owner pid, not handed TextEdit's menu bar.
    #[test]
    fn owner_detects_out_of_process_panel_host() {
        let windows = vec![
            window(41, 800, "TextEdit"),
            window(42, 900, "Open and Save Panel Service"),
        ];
        assert_eq!(
            resolve_window_owner_in(&windows, 800, 42),
            WindowOwner::ForeignPid {
                owner_pid: 900,
                owner_app_name: "Open and Save Panel Service".into(),
            }
        );
    }

    #[test]
    fn owner_is_unknown_for_fabricated_id() {
        let windows = vec![window(42, 800, "TextEdit")];
        assert_eq!(
            resolve_window_owner_in(&windows, 800, 0xFFFF_FFF0),
            WindowOwner::Unknown
        );
    }

    #[test]
    fn owner_is_unknown_for_zero_id() {
        // kCGNullWindowID is never a real window number.
        let windows = vec![window(42, 800, "TextEdit")];
        assert_eq!(
            resolve_window_owner_in(&windows, 800, 0),
            WindowOwner::Unknown
        );
    }

    #[test]
    fn owner_is_unknown_after_the_window_closes() {
        // Stale id: it was enumerated once, then the panel was dismissed.
        let before = vec![window(42, 900, "Open and Save Panel Service")];
        assert_eq!(
            resolve_window_owner_in(&before, 900, 42),
            WindowOwner::SamePid
        );
        assert_eq!(resolve_window_owner_in(&[], 900, 42), WindowOwner::Unknown);
    }
}
