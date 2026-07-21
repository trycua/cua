//! macOS `health_report` provider.
//!
//! Implements [`HealthCheckProvider`] for darwin. Reuses the existing
//! TCC plumbing in `crate::permissions::status` rather than
//! re-implementing it — the goal of the tool is one stable diagnostic
//! call, not duplicate health logic.
//!
//! Schema contract + filter / rollup logic lives in
//! `cua_driver_core::health_report`. This file owns the platform
//! probes only.

use async_trait::async_trait;
use cua_driver_core::health_report::{
    CheckData, CheckEntry, HealthCheckProvider, NAME_AX_CAPABILITY, NAME_BINARY_VERSION,
    NAME_BUNDLE_IDENTITY, NAME_PLATFORM_SUPPORTED, NAME_SCREEN_CAPTURE_CAPABILITY,
    NAME_SESSION_ACTIVE, NAME_TCC_ACCESSIBILITY, NAME_TCC_SCREEN_RECORDING,
};

use crate::permissions::status::{accessibility_granted, current_status, screen_recording_granted};

/// macOS run order, in the order consumers see them reflected back in
/// `report.checks[]`. Matches the Swift PR #1905 contract.
pub const MACOS_CHECK_NAMES: &[&str] = &[
    NAME_BINARY_VERSION,
    NAME_PLATFORM_SUPPORTED,
    NAME_SESSION_ACTIVE,
    NAME_BUNDLE_IDENTITY,
    NAME_TCC_ACCESSIBILITY,
    NAME_TCC_SCREEN_RECORDING,
    NAME_AX_CAPABILITY,
    NAME_SCREEN_CAPTURE_CAPABILITY,
];

/// The canonical bundle identifier whose TCC grants matter for the
/// daemon. The `bundle_identity` check passes when the running process
/// reports this id.
pub const CANONICAL_BUNDLE_ID: &str = "com.trycua.driver";

pub struct MacosHealthProvider;

#[async_trait]
impl HealthCheckProvider for MacosHealthProvider {
    fn platform(&self) -> &'static str {
        "darwin"
    }

    fn check_names(&self) -> &'static [&'static str] {
        MACOS_CHECK_NAMES
    }

    async fn run_check(&self, name: &str) -> CheckEntry {
        match name {
            NAME_BINARY_VERSION => check_binary_version(),
            NAME_PLATFORM_SUPPORTED => check_platform_supported(),
            NAME_SESSION_ACTIVE => check_session_active(),
            NAME_BUNDLE_IDENTITY => check_bundle_identity(),
            NAME_TCC_ACCESSIBILITY => check_tcc_accessibility(),
            NAME_TCC_SCREEN_RECORDING => check_tcc_screen_recording(),
            NAME_AX_CAPABILITY => check_ax_capability(),
            NAME_SCREEN_CAPTURE_CAPABILITY => check_screen_capture_capability(),
            // Defensive: the dispatcher only forwards names in
            // `check_names()`, so this branch should be unreachable.
            other => CheckEntry::skip(
                other.to_owned(),
                "Unknown check name (not implemented on this platform).",
            ),
        }
    }
}

// ── Individual checks ────────────────────────────────────────────────────────

pub(crate) fn check_binary_version() -> CheckEntry {
    // CARGO_PKG_VERSION is a compiled-in constant; reaching this code
    // already implies the binary built. Always pass.
    CheckEntry::pass(
        NAME_BINARY_VERSION,
        format!("cua-driver {}", env!("CARGO_PKG_VERSION")),
    )
}

pub(crate) fn check_platform_supported() -> CheckEntry {
    // Reaching this code path on macOS implies a supported platform —
    // the macOS build is gated behind `cfg(target_os = "macos")`.
    let os_version = sw_vers_product_version().unwrap_or_else(|| "unknown".to_owned());
    let arch = arch_label();
    CheckEntry::pass(
        NAME_PLATFORM_SUPPORTED,
        format!("macOS {os_version} ({arch})"),
    )
    .with_data(CheckData {
        os_version: Some(os_version),
        architecture: Some(arch.to_owned()),
        ..Default::default()
    })
}

pub(crate) fn check_session_active() -> CheckEntry {
    // We are servicing this MCP call, so by construction the session
    // is up. The check exists so consumers can hard-code a canonical
    // "is the server reachable?" signal in a fixed shape.
    CheckEntry::pass(NAME_SESSION_ACTIVE, "MCP session is active.")
}

pub(crate) fn check_bundle_identity() -> CheckEntry {
    let bid = current_bundle_identifier();
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_default();

    let is_correct = bid.as_deref() == Some(CANONICAL_BUNDLE_ID);
    let data = CheckData {
        bundle_identifier: bid.clone(),
        executable_path: if exe.is_empty() { None } else { Some(exe) },
        ..Default::default()
    };
    if is_correct {
        return CheckEntry::pass(
            NAME_BUNDLE_IDENTITY,
            format!("Bundle is {CANONICAL_BUNDLE_ID}."),
        )
        .with_data(data);
    }
    let (message, hint) = match bid.as_deref() {
        None | Some("") => (
            "Process has no CFBundleIdentifier.".to_owned(),
            format!(
                "Run the binary inside CuaDriver.app so TCC grants attribute correctly. \
                 Start the daemon with `open -n -g -a CuaDriver --args serve` and \
                 connect via `cua-driver mcp`."
            ),
        ),
        Some(other) => (
            format!("Bundle is {other}, not {CANONICAL_BUNDLE_ID}."),
            format!(
                "TCC grants will be attributed to {other}, not the cua-driver daemon. \
                 Run via `cua-driver mcp` (auto-relaunches inside CuaDriver.app) or \
                 start the daemon manually: `open -n -g -a CuaDriver --args serve`."
            ),
        ),
    };
    CheckEntry::fail(NAME_BUNDLE_IDENTITY, message, hint).with_data(data)
}

fn check_tcc_accessibility() -> CheckEntry {
    // Reuse the shared probe — do not duplicate AXIsProcessTrusted.
    let status = current_status();
    let data = CheckData {
        bundle_identifier: current_bundle_identifier(),
        ..Default::default()
    };
    if status.accessibility {
        return CheckEntry::pass(NAME_TCC_ACCESSIBILITY, "Accessibility is granted.")
            .with_data(data);
    }
    CheckEntry::fail(
        NAME_TCC_ACCESSIBILITY,
        "Accessibility is NOT granted for this process.",
        "Grant Accessibility to CuaDriver.app in System Settings → Privacy & Security → \
         Accessibility. If the process bundle is not com.trycua.driver (see bundle_identity), \
         the grant must target the responsible app — restart via `cua-driver mcp` to relaunch \
         inside CuaDriver.app.",
    )
    .with_data(data)
}

fn check_tcc_screen_recording() -> CheckEntry {
    let granted = screen_recording_granted();
    let data = CheckData {
        bundle_identifier: current_bundle_identifier(),
        ..Default::default()
    };
    if granted {
        return CheckEntry::pass(NAME_TCC_SCREEN_RECORDING, "Screen Recording is granted.")
            .with_data(data);
    }
    CheckEntry::fail(
        NAME_TCC_SCREEN_RECORDING,
        "Screen Recording is NOT granted for this process.",
        "Grant Screen Recording to CuaDriver.app in System Settings → Privacy & Security → \
         Screen Recording. The grant is attributed to the responsible process — see \
         bundle_identity to confirm the right binary is being prompted.",
    )
    .with_data(data)
}

fn check_ax_capability() -> CheckEntry {
    // The AX trust gate is the same kAXIsProcessTrusted probe
    // `tcc_accessibility` runs, but ax_capability is the consumer-
    // facing "can I actually drive UI" signal. We separate them so a
    // future change (e.g. capability degraded without TCC denial)
    // doesn't break either contract.
    if accessibility_granted() {
        return CheckEntry::pass(NAME_AX_CAPABILITY, "AX is trusted and reachable.");
    }
    CheckEntry::fail(
        NAME_AX_CAPABILITY,
        "AX is not trusted; UI inspection and event posting will fail.",
        "Resolve tcc_accessibility first — AX capability follows directly from the \
         Accessibility TCC grant.",
    )
}

fn check_screen_capture_capability() -> CheckEntry {
    // SCShareableContent::get() is not a read-only probe on recent macOS:
    // Tahoe may display the separate private-window-picker bypass consent.
    // `health_report` is declared read-only, so fail closed to an explicit
    // unknown/skip state. `permissions grant` is the sole flow allowed to
    // explain, request, and verify direct capture.
    CheckEntry::skip(
        NAME_SCREEN_CAPTURE_CAPABILITY,
        "Direct ScreenCaptureKit readiness was not probed because health_report is read-only; run `cua-driver permissions grant` to request and verify it explicitly.",
    )
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Read the running process's `CFBundleIdentifier` via CoreFoundation.
/// Returns `None` when the process has no associated bundle (e.g.
/// running the bare binary outside `CuaDriver.app`).
fn current_bundle_identifier() -> Option<String> {
    use core_foundation::base::TCFType;
    use core_foundation::bundle::{CFBundle, CFBundleGetMainBundle};
    use core_foundation::string::CFString;

    unsafe {
        let raw = CFBundleGetMainBundle();
        if raw.is_null() {
            return None;
        }
        // CFBundleGetMainBundle returns a borrowed reference; wrap it
        // without retaining and read the bundle identifier. `bundle`
        // intentionally doesn't drop the underlying CF object.
        let bundle = CFBundle::wrap_under_get_rule(raw);
        let id_ref = core_foundation::bundle::CFBundleGetIdentifier(bundle.as_concrete_TypeRef());
        if id_ref.is_null() {
            return None;
        }
        let id = CFString::wrap_under_get_rule(id_ref);
        let s = id.to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

/// `sw_vers -productVersion` — e.g. "14.5". Used in the
/// `platform_supported` message; failure falls back to "unknown".
fn sw_vers_product_version() -> Option<String> {
    let output = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// `arm64` / `x86_64`. Matches the label scheme used by `telemetry::arch`.
fn arch_label() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::health_report::{CheckStatus, HealthReportTool};
    use cua_driver_core::tool::Tool;
    use std::sync::Arc;

    #[test]
    fn binary_version_always_passes() {
        let entry = check_binary_version();
        assert_eq!(entry.status, CheckStatus::Pass);
        assert!(
            entry.message.contains("cua-driver "),
            "message must include the binary version"
        );
    }

    #[test]
    fn platform_supported_carries_os_and_arch_data() {
        let entry = check_platform_supported();
        assert_eq!(entry.status, CheckStatus::Pass);
        let data = entry.data.expect("data block expected");
        assert!(data.os_version.is_some(), "os_version must be set");
        assert!(data.architecture.is_some(), "architecture must be set");
    }

    #[test]
    fn session_active_passes() {
        let entry = check_session_active();
        assert_eq!(entry.status, CheckStatus::Pass);
    }

    #[test]
    fn screen_capture_capability_is_not_probed_by_read_only_health_report() {
        let entry = check_screen_capture_capability();
        assert_eq!(entry.status, CheckStatus::Skip);
        assert!(entry.message.contains("permissions grant"));
    }

    #[test]
    fn bundle_identity_in_test_host_fails_with_full_shape() {
        // The Rust test binary runs outside CuaDriver.app, so its
        // bundle id is either absent or not com.trycua.driver. Either
        // way the documented fail-mode shape applies: message + hint
        // + data + (when bid is present) bundle_identifier surfaced.
        //
        // This is the same fixture pattern the Swift PR used: the
        // test environment naturally reproduces the canonical
        // attribution-drift fail mode without any mocking.
        let entry = check_bundle_identity();
        assert_eq!(
            entry.status,
            CheckStatus::Fail,
            "expected bundle_identity to fail in the test host environment"
        );
        assert!(!entry.message.is_empty());
        assert!(
            entry.hint.is_some(),
            "fail entries must carry a remediation hint"
        );
        let data = entry.data.expect("fail entries must carry diagnostic data");
        // executable_path is always available when current_exe()
        // works; surface it so consumers can locate the wrong binary.
        assert!(
            data.executable_path.is_some(),
            "executable_path must be set so consumers can identify the wrong binary"
        );
    }

    // End-to-end through the dispatcher — checks every macOS canonical
    // name appears in the response and the response shape matches the
    // documented contract.
    #[tokio::test]
    async fn invoke_full_run_produces_macos_check_set() {
        let provider = Arc::new(MacosHealthProvider);
        let tool = HealthReportTool::new(provider);
        let result = tool.invoke(serde_json::json!({})).await;
        assert!(
            result.is_error.is_none(),
            "health_report must never set isError"
        );
        let structured = result.structured_content.expect("structured payload");
        assert_eq!(structured["platform"], "darwin");
        assert_eq!(structured["schema_version"], "1");
        let names: Vec<&str> = structured["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        // Every documented macOS check appears, in declared order.
        let expected: Vec<&str> = MACOS_CHECK_NAMES.to_vec();
        assert_eq!(names, expected);
    }
}
