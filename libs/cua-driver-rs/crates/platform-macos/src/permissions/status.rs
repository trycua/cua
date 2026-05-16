//! Low-level TCC permission status probes.
//!
//! Mirrors Swift `Permissions.currentStatus()` / `Permissions.requestAccessibility()`
//! / `Permissions.requestScreenRecording()` — bare booleans, no UI.  Used by
//! both the `check_permissions` MCP tool and the startup permissions gate
//! (`super::gate`).

/// Snapshot of which TCC grants are active for the current process.
///
/// Field naming matches the JSON shape used by the `check_permissions`
/// MCP tool: `accessibility` + `screen_recording`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PermissionsStatus {
    pub accessibility: bool,
    #[serde(rename = "screen_recording")]
    pub screen_recording: bool,
}

impl PermissionsStatus {
    /// True when **both** required TCC grants are active.
    pub fn all_granted(self) -> bool {
        self.accessibility && self.screen_recording
    }
}

/// Read the live TCC status for both required grants.  Cheap to call
/// repeatedly — `AXIsProcessTrusted` is a quick C function and the
/// screen recording probe falls back to the window-enumeration heuristic
/// when `CGPreflightScreenCaptureAccess` reports false.
///
/// Mirrors Swift `Permissions.currentStatus()`.  Difference: Swift uses
/// `SCShareableContent.excludingDesktopWindows` (ScreenCaptureKit) for
/// the screen recording probe, which is unavailable from Rust without
/// large bindings — same caveat documented in `check_permissions.rs`.
pub fn current_status() -> PermissionsStatus {
    PermissionsStatus {
        accessibility: accessibility_granted(),
        screen_recording: screen_recording_granted(),
    }
}

/// Live AX trust state — `AXIsProcessTrusted()`.
pub fn accessibility_granted() -> bool {
    unsafe { crate::ax::bindings::AXIsProcessTrusted() }
}

/// Best-effort screen recording probe.  Uses `CGPreflightScreenCaptureAccess`
/// then falls back to the window-enumeration heuristic for parity with
/// the existing `check_permissions` tool — see that file for rationale.
pub fn screen_recording_granted() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    if unsafe { CGPreflightScreenCaptureAccess() } {
        return true;
    }
    !crate::windows::all_windows().is_empty()
}

/// Raise the Accessibility TCC prompt if not yet granted.  No-op when
/// already active.  Mirrors Swift `Permissions.requestAccessibility()`.
pub fn request_accessibility() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let val = CFBoolean::true_value();
    let options =
        CFDictionary::from_CFType_pairs(&[(key.as_CFType(), val.as_CFType())]);
    unsafe {
        crate::ax::bindings::AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}

/// Raise the Screen Recording TCC prompt if not yet granted.
/// Mirrors Swift `Permissions.requestScreenRecording()`.
pub fn request_screen_recording() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    unsafe { CGRequestScreenCaptureAccess() }
}
