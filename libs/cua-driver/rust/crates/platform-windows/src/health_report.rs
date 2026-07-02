//! Windows `health_report` provider.
//!
//! Implements [`cua_driver_core::health_report::HealthCheckProvider`]
//! for `win32`. Reuses the existing UIA + DXGI probes — see
//! [`crate::diagnostics::ui_automation_available`] for AX and a fresh
//! `D3D11CreateDevice` probe for screen capture.
//!
//! The TCC / bundle checks emit `status: "skip"` with `message: "not
//! applicable on Windows"` rather than `fail`; Windows has no TCC.

use async_trait::async_trait;
use cua_driver_core::health_report::{
    CheckData, CheckEntry, HealthCheckProvider, NAME_AX_CAPABILITY, NAME_BINARY_VERSION,
    NAME_BUNDLE_IDENTITY, NAME_PLATFORM_SUPPORTED, NAME_SCREEN_CAPTURE_CAPABILITY,
    NAME_SESSION_ACTIVE, NAME_TCC_ACCESSIBILITY, NAME_TCC_SCREEN_RECORDING,
};

/// Windows canonical check names — the same Swift-PR contract, with
/// TCC/bundle entries surfaced as `skip("not applicable on Windows")`
/// rather than `fail`. Skipped entries still appear so cross-platform
/// consumers see a complete check map.
pub const WINDOWS_CHECK_NAMES: &[&str] = &[
    NAME_BINARY_VERSION,
    NAME_PLATFORM_SUPPORTED,
    NAME_SESSION_ACTIVE,
    NAME_BUNDLE_IDENTITY,
    NAME_TCC_ACCESSIBILITY,
    NAME_TCC_SCREEN_RECORDING,
    NAME_AX_CAPABILITY,
    NAME_SCREEN_CAPTURE_CAPABILITY,
];

pub struct WindowsHealthProvider;

#[async_trait]
impl HealthCheckProvider for WindowsHealthProvider {
    fn platform(&self) -> &'static str {
        "win32"
    }

    fn check_names(&self) -> &'static [&'static str] {
        WINDOWS_CHECK_NAMES
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
    // The Windows build is gated behind `cfg(target_os = "windows")`,
    // so reaching this code path already implies a supported platform.
    let arch = arch_label();
    let os_version = ver_string().unwrap_or_else(|| "unknown".to_owned());
    CheckEntry::pass(
        NAME_PLATFORM_SUPPORTED,
        format!("Windows {os_version} ({arch})"),
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

/// Standard "this category does not exist on Windows" entry. Used for
/// `tcc_*` and `bundle_identity` so cross-platform consumers can rely
/// on a known answer rather than the entry being missing.
fn skip_not_applicable(name: &str) -> CheckEntry {
    CheckEntry::skip(name.to_owned(), "not applicable on Windows".to_owned())
}

async fn check_ax_capability() -> CheckEntry {
    // CoCreateInstance is synchronous and can take a few ms; keep the
    // async runtime responsive by punting to spawn_blocking.
    let result = tokio::task::spawn_blocking(crate::diagnostics::ui_automation_available)
        .await
        .unwrap_or(Err("UIA probe task panicked".to_owned()));
    match result {
        Ok(()) => CheckEntry::pass(
            NAME_AX_CAPABILITY,
            "UIAutomation is reachable; UI tree walking will succeed.",
        ),
        Err(detail) => CheckEntry::fail(
            NAME_AX_CAPABILITY,
            "UIAutomation is not reachable; AX-driven tools will fail.",
            "If running under Session 0 (Windows service or SSH-launched), re-run from \
             an interactive logon (RDP, console, or a scheduled task in the user's session). \
             Session 0 has no attached desktop, so the UIA COM server cannot be activated.",
        )
        .with_data(CheckData {
            error_detail: Some(detail),
            ..Default::default()
        }),
    }
}

async fn check_screen_capture_capability() -> CheckEntry {
    let result = tokio::task::spawn_blocking(probe_d3d11_device)
        .await
        .unwrap_or(Err("D3D11 probe task panicked".to_owned()));
    match result {
        Ok(()) => CheckEntry::pass(
            NAME_SCREEN_CAPTURE_CAPABILITY,
            "D3D11 device reachable; Windows Graphics Capture will succeed.",
        ),
        Err(detail) => CheckEntry::fail(
            NAME_SCREEN_CAPTURE_CAPABILITY,
            "D3D11 device probe failed.",
            "Confirm a hardware-accelerated graphics adapter is present and current drivers \
             are installed. In Session 0 / headless contexts, D3D hardware may be unavailable \
             — set `include_screenshot:false` on `get_window_state` to skip screen capture entirely.",
        )
        .with_data(CheckData {
            error_detail: Some(detail),
            ..Default::default()
        }),
    }
}

// ── Probes ───────────────────────────────────────────────────────────────────

/// Try to create a hardware D3D11 device + BGRA-capable feature level
/// 11.0 — exactly what the WGC capture path needs.
#[cfg(target_os = "windows")]
fn probe_d3d11_device() -> Result<(), String> {
    use windows::Win32::Graphics::{
        Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0},
        Direct3D11::{
            D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
        },
    };
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    let result = unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE(std::ptr::null_mut()),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    };
    result.map_err(|e| format!("D3D11CreateDevice: {e}"))?;
    // Drop the COM interfaces explicitly so Release runs on this thread.
    drop(context);
    drop(device);
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn probe_d3d11_device() -> Result<(), String> {
    Err("not a Windows host".to_owned())
}

/// `cmd /c ver` — e.g. "Microsoft Windows [Version 10.0.19045.4170]".
/// Fails fast on non-Windows — returns `None`.
fn ver_string() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("cmd").args(["/c", "ver"]).output().ok()?;
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
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
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
            assert_eq!(entry.message, "not applicable on Windows");
        }
    }

    // End-to-end through the dispatcher: every Windows canonical name
    // appears in the response, in declared order. Run on every host so
    // CI matrices that don't target Windows still verify the schema
    // shape — the platform-skipped entries are deterministic regardless
    // of host.
    #[tokio::test]
    async fn invoke_full_run_produces_windows_check_set() {
        let provider = Arc::new(WindowsHealthProvider);
        let tool = HealthReportTool::new(provider);
        let result = tool.invoke(serde_json::json!({})).await;
        assert!(
            result.is_error.is_none(),
            "health_report must never set isError"
        );
        let structured = result.structured_content.expect("structured payload");
        assert_eq!(structured["platform"], "win32");
        assert_eq!(structured["schema_version"], "1");
        let names: Vec<&str> = structured["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        let expected: Vec<&str> = WINDOWS_CHECK_NAMES.to_vec();
        assert_eq!(names, expected);

        // Cross-platform consumers depend on these being skipped (not
        // failed) on Windows — that's the documented "not applicable"
        // shape.
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
                "{name} must be skipped on Windows, got {:?}",
                entry["status"]
            );
            assert_eq!(entry["message"], "not applicable on Windows");
        }
    }
}
