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
    /// Active Space on the display WindowServer associates with this window.
    /// This can differ between windows when displays use independent Spaces.
    pub current_space_id: Option<u64>,
    pub on_current_space: Option<bool>,
    pub space_ids: Option<Vec<u64>>,
}

pub(crate) struct WindowEnumeration {
    pub(crate) windows: Vec<WindowInfo>,
    pub(crate) current_space_id: Option<u64>,
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

/// Enumerate all layer-0 windows (including off-screen).
pub fn all_windows() -> Vec<WindowInfo> {
    enumerate_windows(WindowQuery::desktop()).windows
}

/// Enumerate only on-screen layer-0 windows.
pub fn visible_windows() -> Vec<WindowInfo> {
    enumerate_windows(WindowQuery::desktop().on_screen_only()).windows
}

/// Which CGWindow layers an enumeration admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayerFilter {
    /// Normal application windows only — what a whole-desktop listing reports,
    /// because tooltips, popovers, the Dock and every NSMenu window would
    /// swamp a caller that didn't name a process.
    ZeroOnly,
    /// Every layer, accessory windows included.
    AnyLayer,
}

/// Whether an enumeration resolves per-window Space attribution via SkyLight.
///
/// Independent of [`LayerFilter`]: a layer never decides whether Space data is
/// looked up. Identity lookups skip it because they answer "does this id
/// exist?" on hot input paths where per-window SkyLight round-trips are pure
/// cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpaceLookup {
    Resolve,
    Skip,
}

/// One description of a window enumeration. Layer selection, pid filtering,
/// on-screen filtering and Space lookup are independent axes; every caller —
/// `list_windows`, `launch_app`, main-window resolution, identity lookup —
/// goes through [`enumerate_windows`] with one of these.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WindowQuery {
    pub(crate) on_screen_only: bool,
    pub(crate) layers: LayerFilter,
    pub(crate) pid: Option<i32>,
    pub(crate) spaces: SpaceLookup,
}

impl WindowQuery {
    /// Whole-desktop listing: layer 0 only, Space attribution resolved.
    pub(crate) fn desktop() -> Self {
        Self {
            on_screen_only: false,
            layers: LayerFilter::ZeroOnly,
            pid: None,
            spaces: SpaceLookup::Resolve,
        }
    }

    /// Everything one process owns, on every layer, Space attribution
    /// resolved. Admitting every layer is what makes an app whose entire UI
    /// is an accessory window (floating panel, HUD, SwiftUI onboarding)
    /// reachable instead of looking closed (issue #1451): a caller that
    /// already named the process is not at risk of being swamped by the
    /// desktop-wide accessory noise.
    pub(crate) fn for_pid(pid: i32) -> Self {
        Self {
            on_screen_only: false,
            layers: LayerFilter::AnyLayer,
            pid: Some(pid),
            spaces: SpaceLookup::Resolve,
        }
    }

    /// Identity lookup: "does this CGWindowID exist, and who owns it?" — every
    /// layer, no Space resolution. Keeping the layer filter off identity
    /// lookup is what lets `get_window_state` tell "no such window" apart
    /// from "exists, but is not a layer-0 window" (issue #2237).
    fn identity() -> Self {
        Self {
            on_screen_only: false,
            layers: LayerFilter::AnyLayer,
            pid: None,
            spaces: SpaceLookup::Skip,
        }
    }

    pub(crate) fn on_screen_only(mut self) -> Self {
        self.on_screen_only = true;
        self
    }

    pub(crate) fn skip_spaces(mut self) -> Self {
        self.spaces = SpaceLookup::Skip;
        self
    }

    /// Whether an enumerated window survives this query's filters.
    fn admits(&self, layer: i32, pid: i32) -> bool {
        let layer_ok = self.layers == LayerFilter::AnyLayer || layer == 0;
        let pid_ok = self.pid.is_none_or(|wanted| wanted == pid);
        layer_ok && pid_ok
    }
}

/// The pid's windows with layer-0 preferred: when the process has any layer-0
/// window, accessory windows stay out (existing behavior for every normal
/// app); when it has none, the accessory windows are all there is to return.
pub(crate) fn prefer_layer0(windows: Vec<WindowInfo>) -> Vec<WindowInfo> {
    if windows.iter().any(|w| w.layer == 0) {
        windows.into_iter().filter(|w| w.layer == 0).collect()
    } else {
        windows
    }
}

pub(crate) fn enumerate_windows(query: WindowQuery) -> WindowEnumeration {
    use core_foundation::{
        array::CFArray,
        base::{CFGetTypeID, CFTypeRef, TCFType},
        boolean::CFBoolean,
        dictionary::CFDictionary,
        number::CFNumber,
        string::CFString,
    };
    use std::os::raw::c_void;

    let space_query = (query.spaces == SpaceLookup::Resolve)
        .then(crate::input::skylight::SpaceQuery::new)
        .flatten();
    let current_space_id = space_query
        .as_ref()
        .and_then(|query| query.current_space_id());

    let options = if query.on_screen_only {
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements
    } else {
        kCGWindowListExcludeDesktopElements
    };
    let raw_ref = unsafe { CGWindowListCopyWindowInfo(options, kCGNullWindowID) };
    if raw_ref.is_null() {
        return WindowEnumeration {
            windows: vec![],
            current_space_id,
        };
    }

    let raw: CFArray<CFTypeRef> = unsafe { CFArray::wrap_under_create_rule(raw_ref as _) };
    let total = raw.len() as usize;
    let mut results = Vec::new();

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

        if !query.admits(layer, pid) {
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

        results.push(WindowInfo {
            window_id,
            pid,
            app_name,
            title,
            bounds,
            layer,
            z_index,
            is_on_screen,
            current_space_id: None,
            on_current_space: None,
            space_ids: None,
        });
    }

    if let Some(query) = &space_query {
        resolve_space_metadata(&mut results, query);
    }

    WindowEnumeration {
        windows: results,
        current_space_id,
    }
}

/// Where per-window Space attribution comes from. [`SpaceQuery`] in
/// production; a fixture in tests, so the attribution pipeline is testable
/// without a WindowServer.
///
/// [`SpaceQuery`]: crate::input::skylight::SpaceQuery
pub(crate) trait SpaceMetadataSource {
    fn space_ids(&self, window_id: u32) -> Option<Vec<u64>>;
    fn display_space(&self, window_id: u32) -> Option<u64>;
}

impl SpaceMetadataSource for crate::input::skylight::SpaceQuery {
    fn space_ids(&self, window_id: u32) -> Option<Vec<u64>> {
        self.window_space_ids(window_id)
    }

    fn display_space(&self, window_id: u32) -> Option<u64> {
        self.current_space_for_window(window_id)
    }
}

/// Attribute Spaces to every window in the enumeration, whatever its layer.
/// Layer selection and Space lookup are independent: a pid-filtered
/// enumeration admits accessory layers *and* keeps Space metadata on its
/// layer-0 records.
pub(crate) fn resolve_space_metadata(
    windows: &mut [WindowInfo],
    source: &impl SpaceMetadataSource,
) {
    for window in windows {
        let space_ids = source.space_ids(window.window_id);
        let display_space_id = space_ids
            .as_ref()
            .and_then(|_| source.display_space(window.window_id));
        apply_window_space_metadata(window, space_ids, display_space_id);
    }
}

fn apply_window_space_metadata(
    window: &mut WindowInfo,
    space_ids: Option<Vec<u64>>,
    current_space_id: Option<u64>,
) {
    window.on_current_space = window_on_current_space(space_ids.as_deref(), current_space_id);
    window.current_space_id = current_space_id;
    window.space_ids = space_ids;
}

fn window_on_current_space(
    space_ids: Option<&[u64]>,
    current_space_id: Option<u64>,
) -> Option<bool> {
    Some(space_ids?.contains(&current_space_id?))
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
    enumerate_windows(WindowQuery::identity())
        .windows
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
    resolve_window_owner_in(
        &enumerate_windows(WindowQuery::identity()).windows,
        pid,
        window_id,
    )
}

/// Select the best window_id for a pid. Every layer is enumerated so an app
/// whose entire UI is an accessory window resolves at all; layer-0 windows
/// still win whenever the process has one.
pub fn resolve_main_window_id(pid: i32) -> anyhow::Result<u32> {
    let windows = prefer_layer0(enumerate_windows(WindowQuery::for_pid(pid).skip_spaces()).windows);
    select_main_window(&windows).ok_or_else(|| anyhow::anyhow!("pid {pid} has no windows"))
}

/// Pure form of [`resolve_main_window_id`]'s selection over an
/// already-enumerated window list: frontmost on-screen window, else the
/// largest one.
pub(crate) fn select_main_window(windows: &[WindowInfo]) -> Option<u32> {
    let mut on_screen: Vec<&WindowInfo> = windows.iter().filter(|w| w.is_on_screen).collect();
    if !on_screen.is_empty() {
        on_screen.sort_by_key(|window| std::cmp::Reverse(window.z_index));
        return Some(on_screen[0].window_id);
    }
    windows
        .iter()
        .max_by(|a, b| {
            let area_a = a.bounds.width * a.bounds.height;
            let area_b = b.bounds.width * b.bounds.height;
            area_a
                .partial_cmp(&area_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|w| w.window_id)
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

    #[test]
    fn space_membership_checks_all_spaces_for_a_window() {
        assert_eq!(window_on_current_space(Some(&[2, 4]), Some(4)), Some(true));
        assert_eq!(window_on_current_space(Some(&[2, 4]), Some(3)), Some(false));
    }

    #[test]
    fn space_membership_stays_unknown_without_either_side() {
        assert_eq!(window_on_current_space(None, Some(4)), None);
        assert_eq!(window_on_current_space(Some(&[4]), None), None);
    }

    #[test]
    fn per_window_current_space_is_the_one_used_for_membership() {
        let mut secondary_display_window = window(42, 800, "TextEdit");
        apply_window_space_metadata(&mut secondary_display_window, Some(vec![2, 4]), Some(4));

        assert_eq!(secondary_display_window.current_space_id, Some(4));
        assert_eq!(secondary_display_window.space_ids, Some(vec![2, 4]));
        assert_eq!(secondary_display_window.on_current_space, Some(true));
        assert!(secondary_display_window
            .space_ids
            .as_deref()
            .is_some_and(|spaces| spaces.contains(
                &secondary_display_window
                    .current_space_id
                    .expect("display Space must be present")
            )));
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
            current_space_id: None,
            on_current_space: None,
            space_ids: None,
        }
    }

    fn accessory_window(window_id: u32, pid: i32, app_name: &str, layer: i32) -> WindowInfo {
        let mut w = window(window_id, pid, app_name);
        w.layer = layer;
        w
    }

    #[test]
    fn desktop_query_stays_layer0_and_pid_query_admits_every_layer() {
        let desktop = WindowQuery::desktop();
        assert!(desktop.admits(0, 800));
        assert!(!desktop.admits(3, 800), "the Dock, tooltips and NSMenus must stay out of a whole-desktop listing");

        let for_pid = WindowQuery::for_pid(800);
        assert!(for_pid.admits(0, 800));
        assert!(for_pid.admits(3, 800), "issue #1451: an accessory-only app must be reachable through its pid");
        assert!(!for_pid.admits(0, 801), "another process's windows never leak into a pid query");
    }

    /// A fixed Space attribution table standing in for SkyLight.
    struct SpaceTable(Vec<(u32, Vec<u64>, u64)>);

    impl SpaceMetadataSource for SpaceTable {
        fn space_ids(&self, window_id: u32) -> Option<Vec<u64>> {
            self.0
                .iter()
                .find(|(id, _, _)| *id == window_id)
                .map(|(_, spaces, _)| spaces.clone())
        }

        fn display_space(&self, window_id: u32) -> Option<u64> {
            self.0
                .iter()
                .find(|(id, _, _)| *id == window_id)
                .map(|(_, _, display)| *display)
        }
    }

    /// The regression the any-layer pid path must not reintroduce: admitting
    /// accessory layers into an enumeration must not cost the layer-0 records
    /// their Space attribution. `list_windows({pid})` on TextEdit has to keep
    /// returning space_ids/on_current_space/current_space_id for the document
    /// window while also reporting any accessory window.
    #[test]
    fn space_attribution_is_independent_of_layer_selection() {
        let mut windows = vec![
            window(41, 800, "TextEdit"),
            accessory_window(194, 800, "TextEdit", 3),
        ];
        let table = SpaceTable(vec![(41, vec![2, 4], 4), (194, vec![4], 4)]);

        resolve_space_metadata(&mut windows, &table);

        let document = &windows[0];
        assert_eq!(document.space_ids, Some(vec![2, 4]));
        assert_eq!(document.current_space_id, Some(4));
        assert_eq!(document.on_current_space, Some(true));

        let panel = &windows[1];
        assert_eq!(panel.space_ids, Some(vec![4]));
        assert_eq!(panel.on_current_space, Some(true));
    }

    #[test]
    fn space_attribution_stays_null_only_when_skylight_has_no_record() {
        let mut windows = vec![window(41, 800, "TextEdit")];
        resolve_space_metadata(&mut windows, &SpaceTable(vec![]));
        assert_eq!(windows[0].space_ids, None);
        assert_eq!(windows[0].on_current_space, None);
    }

    #[test]
    fn layer0_still_wins_main_window_resolution_when_the_app_has_one() {
        let windows = prefer_layer0(vec![
            accessory_window(194, 800, "TextEdit", 3),
            window(41, 800, "TextEdit"),
        ]);
        assert_eq!(select_main_window(&windows), Some(41));
    }

    /// Issue #1451: before the shared any-layer pid path, this app resolved to
    /// "pid has no windows" and launch_app reported it as window-less.
    #[test]
    fn accessory_only_app_resolves_its_accessory_window_as_main() {
        let windows = prefer_layer0(vec![accessory_window(194, 800, "OnboardingApp", 3)]);
        assert_eq!(select_main_window(&windows), Some(194));
    }

    /// Live check against WindowServer for the review regression: a
    /// pid-filtered enumeration must keep Space attribution on layer-0
    /// records. Needs a running TextEdit; run with
    /// `TEXTEDIT_PID=$(pgrep -x TextEdit) cargo test -p platform-macos live_pid -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs a live WindowServer and a running TextEdit"]
    fn live_pid_enumeration_keeps_space_attribution_on_layer0() {
        let pid: i32 = std::env::var("TEXTEDIT_PID")
            .expect("set TEXTEDIT_PID")
            .parse()
            .expect("TEXTEDIT_PID must be a pid");
        let windows = enumerate_windows(WindowQuery::for_pid(pid)).windows;
        assert!(!windows.is_empty(), "TextEdit should have windows");
        for w in &windows {
            eprintln!(
                "id={} layer={} {}x{} spaces={:?} on_current={:?} display_space={:?}",
                w.window_id,
                w.layer,
                w.bounds.width,
                w.bounds.height,
                w.space_ids,
                w.on_current_space,
                w.current_space_id
            );
        }
        // The regression bar: layer-0 records must carry exactly the Space
        // attribution the layer-0 desktop listing gives them. (Some windows
        // legitimately have none — SkyLight has no Space record for
        // never-shown windows — so equality, not is_some, is the check.)
        let desktop = enumerate_windows(WindowQuery::desktop()).windows;
        for w in windows.iter().filter(|w| w.layer == 0) {
            let reference = desktop
                .iter()
                .find(|d| d.window_id == w.window_id)
                .expect("layer-0 window must appear in the desktop listing");
            assert_eq!(w.space_ids, reference.space_ids, "id={}", w.window_id);
            assert_eq!(
                w.on_current_space, reference.on_current_space,
                "id={}",
                w.window_id
            );
            assert_eq!(
                w.current_space_id, reference.current_space_id,
                "id={}",
                w.window_id
            );
        }
        assert!(
            windows
                .iter()
                .filter(|w| w.layer == 0 && w.is_on_screen)
                .all(|w| w.space_ids.is_some() && w.on_current_space.is_some()),
            "on-screen layer-0 records must keep Space attribution in a pid enumeration"
        );
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
