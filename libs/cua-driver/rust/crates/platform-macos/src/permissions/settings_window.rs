// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Find the System Settings window the permission guide docks under.
//!
//! Reads only window-server metadata (`CGWindowListCopyWindowInfo`: owner
//! pid, layer, bounds), which needs no permission, so the guide can follow
//! System Settings before Accessibility or Screen Recording is on. System
//! Settings is matched by bundle identifier, never by its name, which macOS
//! translates.

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSRect;

use super::dock::{self, Rect, Screen};

const SETTINGS_BUNDLE_ID: &str = "com.apple.systempreferences";
/// Smaller windows (sheets, popovers, the drag image) are not the main
/// window.
const MIN_SIDE: f64 = 200.0;

/// The frontmost on-screen System Settings window, in AppKit coordinates,
/// or `None` when System Settings has no such window.
pub fn frame() -> Option<Rect> {
    let primary_height = screens().first()?.frame.height;
    settings_pids().into_iter().find_map(|pid| {
        crate::windows::pid_on_screen_windows_any_layer(pid)
            .into_iter()
            .filter(|w| w.layer == 0 && w.bounds.width >= MIN_SIDE && w.bounds.height >= MIN_SIDE)
            .min_by_key(|w| w.z_index)
            .map(|w| {
                dock::from_window_server(
                    Rect::new(w.bounds.x, w.bounds.y, w.bounds.width, w.bounds.height),
                    primary_height,
                )
            })
    })
}

/// Every display, the primary (menu bar) one first, as AppKit lists them.
pub fn screens() -> Vec<Screen> {
    let mut out = Vec::new();
    unsafe {
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        for screen in objects(screens) {
            let frame: NSRect = msg_send![screen, frame];
            let visible: NSRect = msg_send![screen, visibleFrame];
            out.push(Screen {
                frame: rect(frame),
                visible: rect(visible),
            });
        }
    }
    out
}

/// The elements of an `NSArray`, through its enumerator: AppKit may hand
/// back a Swift-bridged array whose `count` is typed `Int`, not
/// `NSUInteger`, which the runtime type check of `msg_send!` rejects.
unsafe fn objects(array: *mut AnyObject) -> Vec<*mut AnyObject> {
    let mut out = Vec::new();
    if array.is_null() {
        return out;
    }
    let enumerator: *mut AnyObject = msg_send![array, objectEnumerator];
    if enumerator.is_null() {
        return out;
    }
    loop {
        let item: *mut AnyObject = msg_send![enumerator, nextObject];
        if item.is_null() {
            return out;
        }
        out.push(item);
    }
}

fn rect(r: NSRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

fn settings_pids() -> Vec<i32> {
    let mut pids = Vec::new();
    unsafe {
        let owned = std::ffi::CString::new(SETTINGS_BUNDLE_ID).unwrap_or_default();
        let bundle_id: *mut AnyObject =
            msg_send![class!(NSString), stringWithUTF8String: owned.as_ptr()];
        let apps: *mut AnyObject = msg_send![
            class!(NSRunningApplication),
            runningApplicationsWithBundleIdentifier: bundle_id
        ];
        for app in objects(apps) {
            let terminated: bool = msg_send![app, isTerminated];
            let pid: i32 = msg_send![app, processIdentifier];
            if pid > 0 && !terminated {
                pids.push(pid);
            }
        }
    }
    pids
}
