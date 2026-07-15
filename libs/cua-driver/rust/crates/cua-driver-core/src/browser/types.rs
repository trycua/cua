//! Shared, serde-friendly value types for the browser-tool surface.
//!
//! These types cross the core ↔ platform-adapter boundary, so they are
//! deliberately conservative: plain data, explicit proof fields (never
//! implicit booleans buried in strings), and `serde` derives throughout
//! so adapters and structured tool output share one shape.

use serde::{Deserialize, Serialize};

/// A rectangle in screen coordinates. Platform adapters MUST normalize
/// native window bounds into the same coordinate space CDP's
/// `Browser.getWindowBounds` reports (device-independent pixels, origin
/// at the top-left of the primary display) before handing them to core —
/// correlation compares the two spaces directly.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether every edge of `self` is within `tolerance` device pixels of
    /// `other`. Tolerance absorbs shadow/inset differences between the
    /// native window server rect and CDP's outer window rect.
    pub fn approx_eq(&self, other: &Rect, tolerance: f64) -> bool {
        (self.x - other.x).abs() <= tolerance
            && (self.y - other.y).abs() <= tolerance
            && (self.width - other.width).abs() <= tolerance
            && (self.height - other.height).abs() <= tolerance
    }
}

/// Rendering-engine family of a classified browser process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserEngineFamily {
    Chromium,
    Gecko,
    Webkit,
    Unknown,
}

/// Platform adapter's verdict on whether a pid is a browser and which
/// engine family it belongs to. `supports_cdp` is the gate for the whole
/// browser-tool route in v1 (Chromium-family only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserClassification {
    pub is_browser: bool,
    pub engine: BrowserEngineFamily,
    /// Human-readable product name, e.g. "Google Chrome", "Microsoft Edge".
    pub product: Option<String>,
    /// Release channel when known, e.g. "stable", "canary".
    pub channel: Option<String>,
    /// Whether this browser can expose a DevTools (CDP) endpoint at all.
    pub supports_cdp: bool,
}

/// How a platform adapter proved that a native window belongs to a pid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeOwnershipMethod {
    /// The OS window server reports the window's owning pid directly.
    WindowServerOwner,
    /// The accessibility tree ties the window element to the pid.
    AccessibilityOwner,
    /// Some other platform-attested mechanism (documented in `detail`).
    PlatformAttested,
}

/// Explicit proof that a native window is owned by a specific process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeOwnershipProof {
    pub method: NativeOwnershipMethod,
    /// Pid the OS attributes the window to. Core refuses when this does
    /// not equal the caller-supplied pid.
    pub owner_pid: i64,
    pub detail: Option<String>,
}

/// Native window metadata used for exact CDP-window correlation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeWindowInfo {
    pub pid: i64,
    pub window_id: u64,
    pub title: String,
    /// Bounds normalized to CDP's coordinate space — see [`Rect`].
    pub bounds: Rect,
    /// Whether the platform can attest that `bounds` are complete and in
    /// the normalized coordinate space. False forces title-only,
    /// read-only correlation even if the numeric values happen to match.
    pub geometry_exact: bool,
    pub ownership: NativeOwnershipProof,
}

/// How a platform adapter proved that a loopback DevTools endpoint is
/// owned by the target browser process (and not an unrelated listener).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointOwnershipMethod {
    /// The listening loopback socket's owning pid was resolved via the
    /// OS socket table and matches the browser pid (or a child of it).
    ListeningSocketPid,
    /// The port was read from the profile's `DevToolsActivePorts` file
    /// belonging to the browser's own user-data dir.
    DevtoolsActivePortsFile,
    /// The driver itself launched the browser with the debugging port,
    /// so ownership is known by construction.
    SpawnedByDriver,
    /// Some other platform-attested mechanism (documented in `detail`).
    PlatformAttested,
}

/// Explicit proof of DevTools-endpoint ownership.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointOwnershipProof {
    pub method: EndpointOwnershipMethod,
    /// Pid the endpoint was attributed to. Core refuses with
    /// `browser_endpoint_owner_mismatch` when this does not equal the
    /// target pid.
    pub owner_pid: i64,
    pub detail: Option<String>,
}

/// A discovered, owned DevTools browser endpoint. `ws_url` must be a
/// loopback `ws://` URL — core validates this before connecting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedEndpoint {
    /// Browser-level WebSocket URL, e.g.
    /// `ws://127.0.0.1:9222/devtools/browser/<uuid>`.
    pub ws_url: String,
    pub http_port: Option<u16>,
    pub ownership: EndpointOwnershipProof,
}

/// A point-in-time identity fingerprint for a process. Used to detect
/// pid reuse between binding and mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessFingerprint {
    pub pid: i64,
    /// OS process start time in platform-native ticks/units. The unit is
    /// opaque to core — only equality matters.
    pub start_time: Option<u64>,
    /// Executable path when the platform can resolve it.
    pub executable: Option<String>,
}

impl ProcessFingerprint {
    /// Whether `other` plausibly identifies the same process instance.
    /// Any field known on both sides must match exactly; a field unknown
    /// on either side is not treated as a match signal, so two
    /// fingerprints that share only the pid still match ONLY if neither
    /// side ever learned a start time / executable. This is the
    /// conservative reading: a platform that can report start times will
    /// always report them, so a disagreement or a newly-missing value
    /// means the process identity can no longer be proven.
    pub fn matches(&self, other: &ProcessFingerprint) -> bool {
        if self.pid != other.pid {
            return false;
        }
        match (&self.start_time, &other.start_time) {
            (Some(a), Some(b)) if a != b => return false,
            (Some(_), None) | (None, Some(_)) => return false,
            _ => {}
        }
        match (&self.executable, &other.executable) {
            (Some(a), Some(b)) if a != b => return false,
            _ => {}
        }
        true
    }
}

/// How well a native window was correlated to a CDP target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingQuality {
    /// Unique bounds correlation or an independently attested one-native /
    /// one-CDP-window cardinality proof. The only quality that permits mutation.
    Exact,
    /// Title-only correlation. Read-only: mutations are refused.
    Heuristic,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_tolerance_is_inclusive() {
        let a = Rect::new(0.0, 0.0, 800.0, 600.0);
        let b = Rect::new(8.0, 0.0, 800.0, 600.0);
        assert!(a.approx_eq(&b, 8.0), "diff exactly at tolerance matches");
        assert!(!a.approx_eq(&b, 7.9), "diff beyond tolerance refuses");
    }

    #[test]
    fn fingerprint_pid_reuse_is_detected() {
        let bound = ProcessFingerprint {
            pid: 100,
            start_time: Some(11111),
            executable: Some("/opt/chrome".into()),
        };
        let same = bound.clone();
        assert!(bound.matches(&same));

        // Same pid, different start time — classic pid reuse.
        let reused = ProcessFingerprint {
            start_time: Some(22222),
            ..bound.clone()
        };
        assert!(!bound.matches(&reused));

        // Start time became unknowable — identity can't be proven anymore.
        let degraded = ProcessFingerprint {
            start_time: None,
            ..bound.clone()
        };
        assert!(!bound.matches(&degraded));

        let other_pid = ProcessFingerprint {
            pid: 101,
            ..bound.clone()
        };
        assert!(!bound.matches(&other_pid));

        let other_exe = ProcessFingerprint {
            executable: Some("/opt/other".into()),
            ..bound
        };
        assert!(!bound.matches(&other_exe));
    }

    #[test]
    fn fingerprint_with_no_known_fields_matches_only_on_pid() {
        let a = ProcessFingerprint {
            pid: 5,
            start_time: None,
            executable: None,
        };
        let b = a.clone();
        assert!(a.matches(&b));
    }
}
