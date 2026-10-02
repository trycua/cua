// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Switching macOS Spaces so a streamed window lands on the normal desktop.
//!
//! A Space desktop view runs as a native-fullscreen window — its own macOS
//! Space (a non-desktop "fullscreen" Space, type != 0). Any window opened while
//! that Space is frontmost (e.g. the `winone-*` stream window) appears trapped
//! inside it. To get back to an ordinary desktop we switch the current Space.
//!
//! Synthetic ⌃← key events do NOT work — macOS ignores injected Space-switch
//! shortcuts (verified: the active Space is unchanged after posting them). The
//! reliable path — the one window managers use — is the private CoreGraphics
//! Spaces API `CGSManagedDisplaySetCurrentSpace`, driven with
//! `CGSCopyManagedDisplaySpaces` to find an ordinary user (type-0) desktop Space
//! to land on. This keeps the fullscreen Space alive in Mission Control (unlike
//! exiting fullscreen, which collapses it). Best-effort: silently does nothing
//! if the private API is unavailable.

/// Switch to the first ordinary user desktop Space when the current Space is a
/// non-desktop (fullscreen) one, so a newly-opened window lands on the desktop
/// instead of trapped in the fullscreen Space. No-op when already on a desktop
/// Space, or on any failure.
#[cfg(target_os = "macos")]
pub fn switch_to_user_desktop_space() {
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::{CFNumber, CFNumberRef};
    use core_foundation::string::{CFString, CFStringRef};

    type CGSConnectionID = i32;
    /// `CGSSpaceType` for an ordinary user desktop Space (fullscreen/system
    /// Spaces report non-zero types).
    const USER_SPACE: i32 = 0;

    #[allow(non_snake_case)]
    extern "C" {
        fn _CGSDefaultConnection() -> CGSConnectionID;
        fn CGSGetActiveSpace(cid: CGSConnectionID) -> u64;
        fn CGSSpaceGetType(cid: CGSConnectionID, space: u64) -> i32;
        fn CGSCopyManagedDisplaySpaces(cid: CGSConnectionID) -> CFArrayRef;
        fn CGSManagedDisplaySetCurrentSpace(cid: CGSConnectionID, display: CFStringRef, space: u64);
    }

    unsafe {
        let cid = _CGSDefaultConnection();
        if cid == 0 {
            return;
        }
        // Already on an ordinary desktop Space → nothing to do.
        if CGSSpaceGetType(cid, CGSGetActiveSpace(cid)) == USER_SPACE {
            return;
        }

        let displays_ref = CGSCopyManagedDisplaySpaces(cid);
        if displays_ref.is_null() {
            return;
        }
        // Array of display dicts, each: { "Display Identifier": CFString,
        // "Spaces": [ { "ManagedSpaceID": CFNumber, ... }, ... ], ... }.
        // Nested containers are read via `wrap_under_get_rule` on the raw refs
        // (the same pattern `window_drag::get_bounds` uses), because `downcast`
        // requires a `ConcreteCFType` which the typed generic forms are not.
        let displays: CFArray<CFType> = CFArray::wrap_under_create_rule(displays_ref);

        for display_index in 0..displays.len() {
            let Some(display_item) = displays.get(display_index) else {
                continue;
            };
            let display: CFDictionary<CFString, CFType> =
                CFDictionary::wrap_under_get_rule(display_item.as_CFTypeRef() as CFDictionaryRef);

            let Some(uuid_value) = display.find(CFString::new("Display Identifier")) else {
                continue;
            };
            let uuid: CFString =
                CFString::wrap_under_get_rule(uuid_value.as_CFTypeRef() as CFStringRef);

            let Some(spaces_value) = display.find(CFString::new("Spaces")) else {
                continue;
            };
            let spaces: CFArray<CFType> =
                CFArray::wrap_under_get_rule(spaces_value.as_CFTypeRef() as CFArrayRef);

            for space_index in 0..spaces.len() {
                let Some(space_item) = spaces.get(space_index) else {
                    continue;
                };
                let space: CFDictionary<CFString, CFType> =
                    CFDictionary::wrap_under_get_rule(space_item.as_CFTypeRef() as CFDictionaryRef);
                let Some(id_value) = space.find(CFString::new("ManagedSpaceID")) else {
                    continue;
                };
                let space_id =
                    CFNumber::wrap_under_get_rule(id_value.as_CFTypeRef() as CFNumberRef);
                let Some(space_id) = space_id.to_i64() else {
                    continue;
                };
                if CGSSpaceGetType(cid, space_id as u64) == USER_SPACE {
                    // First ordinary desktop Space on the first display — the
                    // primary desktop, where the user's windows live.
                    CGSManagedDisplaySetCurrentSpace(
                        cid,
                        uuid.as_concrete_TypeRef(),
                        space_id as u64,
                    );
                    return;
                }
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn switch_to_user_desktop_space() {}
