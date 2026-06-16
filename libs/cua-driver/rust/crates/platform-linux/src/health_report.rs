//! Linux `health_report` provider.
//!
//! Implements [`cua_driver_core::health_report::HealthCheckProvider`]
//! for Linux. Reuses the same probes the Linux `check_permissions`
//! tool runs — X11 connectivity for AX (driven via XSendEvent +
//! AT-SPI) and X11 again for the screen capture path.
//!
//! The TCC / bundle checks emit `status: "skip"` with `message: "not
//! applicable on Linux"`; Linux has no TCC.

use async_trait::async_trait;
use cua_driver_core::health_report::{
    CheckData, CheckEntry, HealthCheckProvider, NAME_AX_CAPABILITY, NAME_BINARY_VERSION,
    NAME_BUNDLE_IDENTITY, NAME_PLATFORM_SUPPORTED, NAME_SCREEN_CAPTURE_CAPABILITY,
    NAME_SESSION_ACTIVE, NAME_TCC_ACCESSIBILITY, NAME_TCC_SCREEN_RECORDING,
};

/// Linux canonical check names — same Swift-PR contract, with TCC /
/// bundle entries surfaced as `skip("not applicable on Linux")`. The
/// full set of entries always appears so cross-platform consumers see
/// a complete check map.
pub const LINUX_CHECK_NAMES: &[&str] = &[
    NAME_BINARY_VERSION,
    NAME_PLATFORM_SUPPORTED,
    NAME_SESSION_ACTIVE,
    NAME_BUNDLE_IDENTITY,
    NAME_TCC_ACCESSIBILITY,
    NAME_TCC_SCREEN_RECORDING,
    NAME_AX_CAPABILITY,
    NAME_SCREEN_CAPTURE_CAPABILITY,
];

pub struct LinuxHealthProvider;

#[async_trait]
impl HealthCheckProvider for LinuxHealthProvider {
    fn platform(&self) -> &'static str {
        "linux"
    }

    fn check_names(&self) -> &'static [&'static str] {
        LINUX_CHECK_NAMES
    }

    async fn run_check(&self, name: &str) -> CheckEntry {
        match name {
            NAME_BINARY_VERSION => check_binary_version(),
            NAME_PLATFORM_SUPPORTED => check_platform_supported(),
            NAME_SESSION_ACTIVE => check_session_active(),
            NAME_BUNDLE_IDENTITY => skip_not_applicable(NAME_BUNDLE_IDENTITY),
            NAME_TCC_ACCESSIBILITY => skip_not_applicable(NAME_TCC_ACCESSIBILITY),
            NAME_TCC_SCREEN_RECORDING => skip_not_applicable(NAME_TCC_SCREEN_RECORDING),
            NAME_AX_CAPABILITY => check_ax_capability().await,
            NAME_SCREEN_CAPTURE_CAPABILITY => check_screen_capture_capability().await,
            other => CheckEntry::skip(
                other.to_owned(),
                "Unknown check name (not implemented on this platform).",
            ),
        }
    }
}

// ── Individual checks ────────────────────────────────────────────────────────

pub(crate) fn check_binary_version() -> CheckEntry {
    CheckEntry::pass(
        NAME_BINARY_VERSION,
        format!("cua-driver {}", env!("CARGO_PKG_VERSION")),
    )
}

pub(crate) fn check_platform_supported() -> CheckEntry {
    let arch = arch_label();
    let os_version = os_release_pretty_name().unwrap_or_else(|| "Linux".to_owned());
    CheckEntry::pass(
        NAME_PLATFORM_SUPPORTED,
        format!("{os_version} ({arch})"),
    )
    .with_data(CheckData {
        os_version: Some(os_version),
        architecture: Some(arch.to_owned()),
        ..Default::default()
    })
}

pub(crate) fn check_session_active() -> CheckEntry {
    CheckEntry::pass(NAME_SESSION_ACTIVE, "MCP session is active.")
}

fn skip_not_applicable(name: &str) -> CheckEntry {
    CheckEntry::skip(name.to_owned(), "not applicable on Linux".to_owned())
}

async fn check_ax_capability() -> CheckEntry {
    // Mirror the existing `check_permissions` Linux probe: X11
    // connectivity is the AX prerequisite (AT-SPI is over D-Bus, but
    // input/readback need an X server). Cheap and side-effect-free.
    let x11_ok = tokio::task::spawn_blocking(probe_x11_connect)
        .await
        .unwrap_or(false);
    if x11_ok {
        return CheckEntry::pass(
            NAME_AX_CAPABILITY,
            "X11 reachable; AT-SPI + XSendEvent input will work.",
        );
    }
    CheckEntry::fail(
        NAME_AX_CAPABILITY,
        "X11 is not reachable; UI inspection and event injection will fail.",
        "Set DISPLAY (X11) — under Wayland, run via XWayland or expose XDG_SESSION_TYPE=x11.",
    )
}

async fn check_screen_capture_capability() -> CheckEntry {
    // On Linux the canonical capture path is X11 GetImage / xwd / scrot
    // — all require an open X11 connection. The probe is the same one
    // we use for ax_capability, but the consumer-facing message and
    // hint differ so future drift between the two doesn't break either
    // contract.
    let x11_ok = tokio::task::spawn_blocking(probe_x11_connect)
        .await
        .unwrap_or(false);
    if x11_ok {
        return CheckEntry::pass(
            NAME_SCREEN_CAPTURE_CAPABILITY,
            "X11 reachable; screen capture path is functional.",
        );
    }
    CheckEntry::fail(
        NAME_SCREEN_CAPTURE_CAPABILITY,
        "X11 is not reachable; screen capture will fail.",
        "Set DISPLAY (X11). Pure Wayland sessions require an XWayland bridge for capture.",
    )
}

// ── Probes ───────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn probe_x11_connect() -> bool {
    x11rb::rust_connection::RustConnection::connect(None).is_ok()
}

#[cfg(not(target_os = "linux"))]
fn probe_x11_connect() -> bool {
    false
}

/// `PRETTY_NAME=` out of `/etc/os-release`. Fails to "Linux" so the
/// downstream message stays human-readable.
fn os_release_pretty_name() -> Option<String> {
    let s = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("PRETTY_NAME=") {
            let v = rest.trim_matches('"').to_owned();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

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
        assert!(entry.message.contains("cua-driver "));
    }

    #[test]
    fn session_active_passes() {
        let entry = check_session_active();
        assert_eq!(entry.status, CheckStatus::Pass);
    }

    #[test]
    fn tcc_and_bundle_are_skipped_with_canonical_message() {
        for name in [
            NAME_TCC_ACCESSIBILITY,
            NAME_TCC_SCREEN_RECORDING,
            NAME_BUNDLE_IDENTITY,
        ] {
            let entry = skip_not_applicable(name);
            assert_eq!(entry.status, CheckStatus::Skip, "{name} must be skipped");
            assert_eq!(entry.message, "not applicable on Linux");
        }
    }

    #[tokio::test]
    async fn invoke_full_run_produces_linux_check_set() {
        let provider = Arc::new(LinuxHealthProvider);
        let tool = HealthReportTool::new(provider);
        let result = tool.invoke(serde_json::json!({})).await;
        assert!(
            result.is_error.is_none(),
            "health_report must never set isError"
        );
        let structured = result.structured_content.expect("structured payload");
        assert_eq!(structured["platform"], "linux");
        assert_eq!(structured["schema_version"], "1");
        let names: Vec<&str> = structured["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        let expected: Vec<&str> = LINUX_CHECK_NAMES.to_vec();
        assert_eq!(names, expected);

        let by_name: std::collections::HashMap<_, _> = structured["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c["name"].as_str().unwrap(), c))
            .collect();
        for name in [
            NAME_TCC_ACCESSIBILITY,
            NAME_TCC_SCREEN_RECORDING,
            NAME_BUNDLE_IDENTITY,
        ] {
            let entry = by_name[name];
            assert_eq!(
                entry["status"], "skip",
                "{name} must be skipped on Linux"
            );
            assert_eq!(entry["message"], "not applicable on Linux");
        }
    }
}
