// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Dragging an app window onto a Space (macOS), on the cua SDK.
//!
//! Detection, the window list and the dragged window's preview are the
//! SDK's (`cua_teleport::ux::window`): a listen-only `CGEventTap` feeds the
//! drag tracker, which reports `start` once the window under the pointer
//! actually follows it, then `move` and `end`. Each `start`/`end` carries
//! the window and its app, classified by the SDK catalog
//! (`capability`: full, install_only, unsupported). The shell forwards them
//! to the renderer as `window-drag` events; the renderer hit-tests its own
//! tiles and opens "Teleport an app…" for that app on release.
//!
//! Each `start` also carries the window's frame at mouse down and now (and
//! some `move`s the frame again): the renderer feeds them, with the cursor,
//! to the app core's drag trigger (`dragTrigger.apply`), which tells a move
//! from a resize and decides when the switcher opens.
//! [`drag_trigger_displays`] is that trigger's geometry, from each screen's
//! safe area and auxiliary top areas.
//!
//! What stays here is app-specific: the Tauri commands, the Accessibility
//! permission prompt, and hiding / restoring the real window while its
//! preview stands in for it.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::AppState;

/// One on-screen window, with its app as the teleport catalog sees it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenWindow {
    /// CoreGraphics window number (for the preview). 0 outside macOS.
    pub window_id: u32,
    /// The app's catalog id (`vscode`, `firefox`, a bundle id).
    pub app_id: String,
    /// The owning application's name.
    pub app_name: String,
    /// The window title (empty without Screen Recording).
    pub window_title: String,
    /// Whether teleport can bring the app up in a Space.
    pub supported: bool,
    /// `full`, `install_only` or `unsupported`.
    pub capability: cua_teleport::ux::Capability,
    /// The owning app's bundle, when known.
    pub bundle_path: Option<String>,
    /// The catalog entry (core JSON), for a preselected picker.
    pub entry: cua_teleport::ux::CatalogEntry,
    /// Icons stream in through `app_icon`; always `None` here.
    pub icon: Option<String>,
}

/// The installed monitor, kept alive for the app's lifetime.
#[derive(Default)]
pub struct WindowDragState {
    monitor: Mutex<Option<cua_teleport::ux::window::WindowDragMonitor>>,
}

/// Where to park a hidden foreign window: slide its whole frame off the left
/// of the global display space so it disappears without minimizing.
#[cfg(any(target_os = "macos", test))]
pub(crate) const OFFSCREEN_MARGIN: f64 = 200.0;

#[cfg(any(target_os = "macos", test))]
pub(crate) fn offscreen_origin(width: f64) -> (f64, f64) {
    (-(width.max(0.0) + OFFSCREEN_MARGIN), 0.0)
}

/// The `window-drag` event payload.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DragPayload {
    phase: cua_teleport::ux::window::DragPhase,
    x: f64,
    y: f64,
    window_id: Option<u32>,
    app_id: Option<String>,
    app_name: Option<String>,
    window_title: Option<String>,
    supported: Option<bool>,
    capability: Option<cua_teleport::ux::Capability>,
    entry: Option<cua_teleport::ux::CatalogEntry>,
    start_frame: Option<cua_teleport::ux::window::Rect>,
    frame: Option<cua_teleport::ux::window::Rect>,
}

/// Maps an SDK event, classifying the dragged window's app with `classify`
/// (the app core in the shell; fixtures in tests).
pub(crate) fn drag_payload(
    e: cua_teleport::ux::window::WindowDragEvent,
    classify: impl Fn(Option<&str>, &str) -> cua_teleport::ux::CatalogEntry,
) -> DragPayload {
    let entry = e
        .window
        .as_ref()
        .map(|w| classify(w.bundle_path.as_deref(), &w.app_name));
    DragPayload {
        phase: e.phase,
        x: e.x,
        y: e.y,
        window_id: e.window.as_ref().map(|w| w.window_id),
        app_id: entry.as_ref().map(|en| en.id.clone()),
        app_name: e.window.as_ref().map(|w| w.app_name.clone()),
        window_title: e.window.as_ref().map(|w| w.title.clone()),
        supported: entry.as_ref().map(|en| en.is_enabled()),
        capability: entry.as_ref().map(|en| en.capability),
        entry,
        start_frame: e.start_frame,
        frame: e.frame,
    }
}

// --- Tauri commands ----------------------------------------------------------

/// Whether the app currently holds the Accessibility permission.
#[tauri::command]
pub fn ax_trusted() -> bool {
    cua_teleport::ux::window::permission_granted()
}

/// Prompt for the Accessibility permission (opens the system pane). Returns
/// the trust state after prompting.
#[tauri::command]
pub fn request_ax_trust() -> bool {
    cua_teleport::ux::window::request_permission().unwrap_or(false)
}

/// The user's windows, each with its app's teleport capability. Off the
/// main thread: the window list and each app's metadata are blocking reads.
/// Always `Ok`: Tauri needs async commands that borrow `State` to return a
/// `Result`.
#[tauri::command]
pub async fn list_open_windows(state: State<'_, AppState>) -> Result<Vec<OpenWindow>, String> {
    let core = state.0.clone();
    Ok(
        tauri::async_runtime::spawn_blocking(move || open_windows(&core))
            .await
            .unwrap_or_default(),
    )
}

fn open_windows(core: &crate::core::AppCore) -> Vec<OpenWindow> {
    let Ok(windows) = cua_teleport::ux::window::list_user_windows() else {
        return vec![];
    };
    windows
        .into_iter()
        .map(|w| {
            let entry = core.teleport_entry_for_window(w.bundle_path.as_deref(), &w.owner);
            OpenWindow {
                window_id: w.window_id,
                app_id: entry.id.clone(),
                app_name: w.owner,
                window_title: w.title,
                supported: entry.is_enabled(),
                capability: entry.capability,
                bundle_path: w.bundle_path,
                entry,
                icon: None,
            }
        })
        .collect()
}

/// A `data:image/png;base64,…` preview of that one window (never the screen
/// or another window), kept in memory. `None` without Screen Recording.
#[tauri::command]
pub async fn capture_window_thumbnail(window_id: u32) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || {
        cua_teleport::ux::capture_thumbnail_png_cached(
            window_id,
            cua_teleport::ux::window::THUMBNAIL_WIDTH,
        )
        .ok()
        .flatten()
        .and_then(|png| cua_teleport::ux::window::png_data_url(&png))
    })
    .await
    .ok()
    .flatten()
}

/// The icon of an app with an open window, keyed by the `appId` from
/// `list_open_windows` (or its name). `None` when no window matches.
/// Always `Ok`, as with `list_open_windows`.
#[tauri::command]
pub async fn app_icon(
    state: State<'_, AppState>,
    app_id: String,
) -> Result<Option<String>, String> {
    let core = state.0.clone();
    Ok(tauri::async_runtime::spawn_blocking(move || {
        let windows = cua_teleport::ux::window::list_user_windows().ok()?;
        let path = windows.into_iter().find_map(|w| {
            let entry = core.teleport_entry_for_window(w.bundle_path.as_deref(), &w.owner);
            (entry.id == app_id || w.owner == app_id)
                .then_some(w.bundle_path)
                .flatten()
        })?;
        cua_teleport::ux::app_icon_png_cached(&path, 36)
            .and_then(|png| cua_teleport::ux::window::png_data_url(&png))
    })
    .await
    .ok()
    .flatten())
}

/// The drag trigger's geometry for every display (the primary first), in
/// global top-left points: the notch's bottom edge from each screen's safe
/// area and auxiliary top areas (the menu bar's height without a notch),
/// and the switcher's frame. Empty off macOS. Synchronous, so Tauri runs it
/// on the main thread, where AppKit's screens are read.
#[tauri::command]
pub fn drag_trigger_displays() -> Vec<cua_spaces_app_core::notch::drag_trigger::DragDisplay> {
    cua_spaces_app_core::notch::drag_trigger::portal_displays(&imp::screen_facts())
}

/// Install the global window-drag monitor if permitted and not already running.
#[tauri::command]
pub fn start_window_drag(app: AppHandle, state: State<'_, WindowDragState>) -> bool {
    ensure_monitor(&app, &state)
}

/// Hide (`hidden = true`) or restore a foreign window during the drag, so its
/// preview can stand in for it. `false` when Accessibility is not permitted
/// or the window could not be resolved.
#[tauri::command]
pub async fn set_foreign_window_hidden(window_id: u32, hidden: bool) -> bool {
    tauri::async_runtime::spawn_blocking(move || imp::set_foreign_window_hidden(window_id, hidden))
        .await
        .unwrap_or(false)
}

/// Install the monitor once. Emits `window-drag-permission {false}` and
/// returns `false` when it cannot run (no Accessibility permission, or not
/// macOS).
pub fn ensure_monitor(app: &AppHandle, state: &WindowDragState) -> bool {
    let mut slot = state.monitor.lock().unwrap_or_else(|p| p.into_inner());
    if slot.is_some() {
        return true;
    }
    if !cua_teleport::ux::window::supported() || !cua_teleport::ux::window::permission_granted() {
        let _ = app.emit("window-drag-permission", false);
        return false;
    }
    let handle = app.clone();
    let started = cua_teleport::ux::window::WindowDragMonitor::start(Box::new(move |e| {
        let core = handle.state::<AppState>().0.clone();
        let payload = drag_payload(e, |bundle, name| {
            core.teleport_entry_for_window(bundle, name)
        });
        let _ = handle.emit("window-drag", payload);
    }));
    match started {
        Ok(m) => {
            *slot = Some(m);
            let _ = app.emit("window-drag-permission", true);
            true
        }
        Err(e) => {
            tracing::warn!("window-drag monitor did not start: {e}");
            let _ = app.emit("window-drag-permission", false);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_teleport::ux::window::{MouseEvent, Rect, WindowDragTracker, WindowInfo};

    fn fixture(x: f64) -> Vec<WindowInfo> {
        vec![WindowInfo {
            window_id: 42,
            pid: 4200,
            owner: "Visual Studio Code".into(),
            title: "project".into(),
            layer: 0,
            alpha: 1.0,
            bounds: Some(Rect {
                x,
                y: 40.0,
                width: 800.0,
                height: 600.0,
            }),
            visible: true,
            bundle_path: None,
        }]
    }

    fn classify(_bundle: Option<&str>, name: &str) -> cua_teleport::ux::CatalogEntry {
        let registry = cua_teleport::ExportRegistry::with_builtin_host(std::sync::Arc::new(
            cua_teleport::FakeHost::new(),
        ));
        cua_teleport::ux::entry_for_name(&registry, name, &Default::default())
    }

    /// A fixture window drag (no real windows) becomes the `window-drag`
    /// payload the renderer reads, with the app's capability and entry.
    #[test]
    fn a_fixture_window_drag_becomes_the_renderer_payload() {
        let mut t = WindowDragTracker::new(fixture(100.0), 1);
        assert!(t.handle(MouseEvent::Down, 150.0, 60.0).is_none());
        *t.source_mut() = fixture(160.0);
        let start = drag_payload(
            t.handle(MouseEvent::Dragged, 210.0, 60.0).unwrap(),
            classify,
        );
        let json = serde_json::to_value(&start).unwrap();
        assert_eq!(json["phase"], "start");
        assert_eq!(json["windowId"], 42);
        assert_eq!(json["appId"], "vscode");
        assert_eq!(json["supported"], true);
        assert_eq!(json["capability"], "install_only");
        assert_eq!(json["entry"]["launch"]["bin"], "code");
        // The frames at mouse down and now, for the core's move/resize test.
        assert_eq!(json["startFrame"]["x"], 100.0);
        assert_eq!(json["frame"]["x"], 160.0);
        assert_eq!(json["frame"]["width"], 800.0);
        let mv = drag_payload(
            t.handle(MouseEvent::Dragged, 220.0, 10.0).unwrap(),
            classify,
        );
        assert!(serde_json::to_value(&mv).unwrap()["appId"].is_null());
        let end = drag_payload(t.handle(MouseEvent::Up, 220.0, 10.0).unwrap(), classify);
        assert_eq!(serde_json::to_value(&end).unwrap()["phase"], "end");
    }

    #[test]
    fn an_unknown_app_is_reported_unsupported() {
        let e = cua_teleport::ux::window::WindowDragEvent {
            phase: cua_teleport::ux::window::DragPhase::Start,
            x: 0.0,
            y: 0.0,
            window: Some(cua_teleport::ux::window::DraggedWindow {
                window_id: 1,
                pid: 1,
                app_name: "Safari".into(),
                title: String::new(),
                bundle_path: None,
            }),
            start_frame: None,
            frame: None,
        };
        let json = serde_json::to_value(drag_payload(e, classify)).unwrap();
        assert_eq!(json["supported"], false);
        assert_eq!(json["capability"], "unsupported");
    }

    #[test]
    fn offscreen_origin_parks_the_whole_frame_left_of_the_display() {
        assert_eq!(offscreen_origin(800.0), (-1000.0, 0.0));
        let (x, _) = offscreen_origin(1440.0);
        assert!(x <= -(1440.0 + OFFSCREEN_MARGIN));
        assert_eq!(offscreen_origin(-5.0), (-OFFSCREEN_MARGIN, 0.0));
    }
}

// --- macOS: hide / restore a foreign window (Accessibility) -----------------

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::sync::{Mutex, OnceLock};

    use core_foundation::array::CFArrayRef;
    use core_foundation::base::{CFType, CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::geometry::{CGPoint, CGSize};

    use super::offscreen_origin;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementSetAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: CFTypeRef,
        ) -> i32;
        fn AXValueCreate(the_type: u32, value_ptr: *const c_void) -> AXValueRef;
        fn AXValueGetValue(value: AXValueRef, the_type: u32, value_ptr: *mut c_void) -> u8;
        // Private but long-stable: maps an AX element to its CGWindowID.
        fn _AXUIElementGetWindow(element: AXUIElementRef, out: *mut u32) -> i32;
    }

    type AXUIElementRef = *mut c_void;
    type AXValueRef = *mut c_void;
    const K_AX_ERROR_SUCCESS: i32 = 0;
    const K_AX_VALUE_CGPOINT_TYPE: u32 = 1;
    const K_AX_VALUE_CGSIZE_TYPE: u32 = 2;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFArrayGetCount(array: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFArrayRef, idx: isize) -> *const c_void;
    }

    fn ax_trusted() -> bool {
        cua_teleport::ux::window::permission_granted()
    }

    // --- Foreign window hide/restore (Accessibility) -------------------------

    /// Original on-screen positions of windows we've parked off-screen, keyed by
    /// CGWindowID so a restore puts each frame back exactly where it was.
    fn hide_store() -> &'static Mutex<HashMap<u32, (f64, f64)>> {
        static STORE: OnceLock<Mutex<HashMap<u32, (f64, f64)>>> = OnceLock::new();
        STORE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// Owning process id for a CGWindowID, or None when the window is gone.
    fn window_pid(window_id: u32) -> Option<i32> {
        cua_teleport::ux::window::list_windows()
            .ok()?
            .into_iter()
            .find(|w| w.window_id == window_id)
            .map(|w| w.pid as i32)
    }

    /// Copy an AX attribute as a CoreFoundation value (auto-released).
    unsafe fn copy_attr(elem: AXUIElementRef, name: &str) -> Option<CFType> {
        let key = CFString::new(name);
        let mut value: CFTypeRef = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(elem, key.as_concrete_TypeRef(), &mut value);
        if err != K_AX_ERROR_SUCCESS || value.is_null() {
            return None;
        }
        Some(CFType::wrap_under_create_rule(value))
    }

    unsafe fn ax_point(value: &CFType) -> Option<(f64, f64)> {
        let mut p = CGPoint::new(0.0, 0.0);
        let ok = AXValueGetValue(
            value.as_CFTypeRef() as AXValueRef,
            K_AX_VALUE_CGPOINT_TYPE,
            &mut p as *mut CGPoint as *mut c_void,
        );
        (ok != 0).then_some((p.x, p.y))
    }

    unsafe fn ax_size(value: &CFType) -> Option<(f64, f64)> {
        let mut s = CGSize::new(0.0, 0.0);
        let ok = AXValueGetValue(
            value.as_CFTypeRef() as AXValueRef,
            K_AX_VALUE_CGSIZE_TYPE,
            &mut s as *mut CGSize as *mut c_void,
        );
        (ok != 0).then_some((s.width, s.height))
    }

    unsafe fn set_position(elem: AXUIElementRef, x: f64, y: f64) -> bool {
        let p = CGPoint::new(x, y);
        let value = AXValueCreate(
            K_AX_VALUE_CGPOINT_TYPE,
            &p as *const CGPoint as *const c_void,
        );
        if value.is_null() {
            return false;
        }
        let key = CFString::new("AXPosition");
        let err = AXUIElementSetAttributeValue(elem, key.as_concrete_TypeRef(), value as CFTypeRef);
        CFRelease(value as CFTypeRef);
        err == K_AX_ERROR_SUCCESS
    }

    /// Park (`hidden`) or restore the AX window matching `window_id` in `app`.
    unsafe fn apply_hidden(app: AXUIElementRef, window_id: u32, hidden: bool) -> bool {
        let Some(windows_val) = copy_attr(app, "AXWindows") else {
            return false;
        };
        let array_ref = windows_val.as_CFTypeRef() as CFArrayRef;
        let count = CFArrayGetCount(array_ref);
        for i in 0..count {
            let elem = CFArrayGetValueAtIndex(array_ref, i) as AXUIElementRef;
            if elem.is_null() {
                continue;
            }
            let mut wid: u32 = 0;
            if _AXUIElementGetWindow(elem, &mut wid) != K_AX_ERROR_SUCCESS || wid != window_id {
                continue;
            }
            if hidden {
                let Some(pos) = copy_attr(elem, "AXPosition").and_then(|v| ax_point(&v)) else {
                    return false;
                };
                let width = copy_attr(elem, "AXSize")
                    .and_then(|v| ax_size(&v))
                    .map(|(w, _)| w)
                    .unwrap_or(0.0);
                hide_store()
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(window_id, pos);
                let (ox, oy) = offscreen_origin(width);
                return set_position(elem, ox, oy);
            }
            let orig = hide_store()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&window_id);
            return match orig {
                Some((ox, oy)) => set_position(elem, ox, oy),
                None => false,
            };
        }
        false
    }

    /// Every screen's facts (the primary first), AppKit coordinates. Empty
    /// off the main thread.
    pub fn screen_facts() -> Vec<cua_spaces_app_core::notch::ScreenFacts> {
        use cua_spaces_app_core::notch::{LogicalRect, ScreenFacts};
        use objc2_app_kit::NSScreen;
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return vec![];
        };
        let rect = |r: objc2_foundation::NSRect| {
            LogicalRect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
        };
        let width = |r: objc2_foundation::NSRect| (r.size.width > 0.0).then_some(r.size.width);
        NSScreen::screens(mtm)
            .iter()
            .map(|s| ScreenFacts {
                frame: rect(s.frame()),
                visible_frame: rect(s.visibleFrame()),
                safe_area_top: s.safeAreaInsets().top,
                aux_left_width: width(s.auxiliaryTopLeftArea()),
                aux_right_width: width(s.auxiliaryTopRightArea()),
            })
            .collect()
    }

    pub fn set_foreign_window_hidden(window_id: u32, hidden: bool) -> bool {
        if !ax_trusted() {
            return false; // Accessibility withheld — caller keeps the ghost only.
        }
        let Some(pid) = window_pid(window_id) else {
            return false;
        };
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() {
                return false;
            }
            let result = apply_hidden(app, window_id, hidden);
            CFRelease(app as CFTypeRef);
            result
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn screen_facts() -> Vec<cua_spaces_app_core::notch::ScreenFacts> {
        vec![]
    }

    pub fn set_foreign_window_hidden(_window_id: u32, _hidden: bool) -> bool {
        false
    }
}
