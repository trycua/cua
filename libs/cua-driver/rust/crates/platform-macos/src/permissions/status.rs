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
/// repeatedly — both probes are quick C-level calls into the system
/// TCC daemon.
///
/// Mirrors Swift `Permissions.currentStatus()`.  Difference: Swift uses
/// `SCShareableContent.excludingDesktopWindows` (ScreenCaptureKit) for
/// the screen recording probe, which is unavailable from Rust without
/// large bindings.  `CGPreflightScreenCaptureAccess` is Apple's
/// documented preflight API for the same grant and is accurate on
/// macOS 11+.
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

/// Live Screen Recording grant state — `CGPreflightScreenCaptureAccess()`.
///
/// This is the only probe.  Earlier versions fell back to
/// `!all_windows().is_empty()` when preflight returned false, on the
/// theory that `CGWindowListCopyWindowInfo` returning real windows
/// implied the grant was active.  That theory is wrong:
/// `CGWindowListCopyWindowInfo` returns window IDs and bounds for **any**
/// process without requiring the Screen Recording grant — only window
/// titles are gated.  The fallback therefore returned `true` on any
/// populated desktop regardless of grant state, which (a) made
/// `check_permissions` report a false positive after `tccutil reset
/// ScreenCapture com.trycua.driver`, and (b) short-circuited the
/// startup permissions gate's prompt for users who had never granted SR.
pub fn screen_recording_granted() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
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
