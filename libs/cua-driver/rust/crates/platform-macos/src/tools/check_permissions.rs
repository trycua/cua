use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{ProtectedResourceOwnership, Tool, ToolDef},
};
use serde_json::Value;
use std::{future::Future, sync::Arc, time::Duration};

use super::ToolState;
use crate::permissions::status::{
    accessibility_granted, request_accessibility, request_screen_recording,
    screen_recording_granted,
};

/// Private argv sentinel shared by the trusted CLI launcher and the public
/// launch_app refusal.
pub const PERMISSIONS_HOST_REQUEST_ARG: &str = "__permissions-host-request";

pub struct CheckPermissionsTool {
    state: Arc<ToolState>,
    /// Set only by the private LaunchServices permission helper. Registered
    /// agent tools retain the legacy enumeration probe.
    native_setup_frame_probe: bool,
}

impl CheckPermissionsTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self {
            state,
            native_setup_frame_probe: false,
        }
    }

    fn for_trusted_permission_setup(state: Arc<ToolState>) -> Self {
        Self {
            state,
            native_setup_frame_probe: true,
        }
    }
}

/// LaunchServices-hosted permission setup entrypoint. This is deliberately
/// not registered as an agent tool or exposed on the daemon socket: the
/// standalone `permissions grant` command launches the app bundle and macOS
/// owns the actual approval UI.
pub async fn request_from_launchservices_host(probe_direct_capture: bool) -> ToolResult {
    // This constructor is the only route that selects a prompt-capable native
    // frame. The public tool constructor remains enumeration-only, and strict
    // startup calls the tool read-only with `prompt:false`.
    let tool = CheckPermissionsTool::for_trusted_permission_setup(Arc::new(ToolState::new(
        false, false, None,
    )));
    tool.invoke(serde_json::json!({
        "prompt": true,
        "probe_direct_capture": probe_direct_capture,
    }))
    .await
}

fn driver_bundle_id_for_executable(executable: &str) -> Option<&'static str> {
    if executable.contains("/CuaDriverLocal.app/Contents/MacOS/") {
        Some("com.trycua.driver.local")
    } else if executable.contains("/CuaDriver.app/Contents/MacOS/") {
        Some("com.trycua.driver")
    } else {
        None
    }
}

/// (A) Real ScreenCaptureKit capability probe — what THIS process can
/// actually capture right now, independent of the CGPreflight cache.
///
/// `CGPreflightScreenCaptureAccess()` (used by `screen_recording_granted`)
/// answers from a per-process cache that goes stale after `tccutil reset`
/// and is unreliable for CLI / child processes — the same finding Peekaboo
/// documents. `SCShareableContent::get()` does a live query: it only
/// returns displays when the answering process can genuinely capture. When
/// it disagrees with the preflight boolean, the preflight one is lying.
fn screen_recording_capturable() -> bool {
    use screencapturekit::prelude::SCShareableContent;
    SCShareableContent::get()
        .map(|c| !c.displays().is_empty())
        .unwrap_or(false)
}

const DIRECT_CAPTURE_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectCaptureProbeResult {
    Ready,
    Unavailable,
    TimedOut,
    Failed,
}

impl DirectCaptureProbeResult {
    fn response_fields(self) -> (Option<bool>, &'static str, Option<Value>) {
        match self {
            Self::Ready => (Some(true), "ready", None),
            Self::Unavailable => (Some(false), "unavailable", None),
            Self::TimedOut => (
                None,
                "timed_out",
                Some(serde_json::json!({
                    "code": "direct_capture_probe_timed_out",
                    "message": "The ScreenCaptureKit capability probe did not complete within 10 seconds.",
                })),
            ),
            Self::Failed => (
                None,
                "probe_failed",
                Some(serde_json::json!({
                    "code": "direct_capture_probe_failed",
                    "message": "The ScreenCaptureKit capability probe could not complete.",
                })),
            ),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct DirectCaptureProvenance {
    backend: Option<&'static str>,
    operation: Option<&'static str>,
    frame_captured: Option<bool>,
    frame_width: Option<u32>,
    frame_height: Option<u32>,
    frame_byte_count: Option<u64>,
    fallback_used: Option<bool>,
}

impl DirectCaptureProvenance {
    fn native_attempt(frame: Option<crate::capture::NativePermissionSetupFrame>) -> Self {
        Self {
            backend: Some(crate::capture::PERMISSION_SETUP_CAPTURE_BACKEND),
            operation: Some(crate::capture::PERMISSION_SETUP_CAPTURE_OPERATION),
            frame_captured: Some(frame.is_some()),
            frame_width: frame.map(|value| value.width),
            frame_height: frame.map(|value| value.height),
            frame_byte_count: frame.map(|value| value.rgba_byte_count),
            // This trusted probe has no shell compatibility call. The CLI can
            // distinguish it from ordinary capture's silent fallback.
            fallback_used: Some(false),
        }
    }
}

async fn run_direct_capture_probe<F>(timeout: Duration, probe: F) -> DirectCaptureProbeResult
where
    F: Future<Output = Result<bool, tokio::task::JoinError>>,
{
    match tokio::time::timeout(timeout, probe).await {
        Ok(Ok(true)) => DirectCaptureProbeResult::Ready,
        Ok(Ok(false)) => DirectCaptureProbeResult::Unavailable,
        Ok(Err(error)) => {
            tracing::warn!(%error, "direct ScreenCaptureKit capability probe failed");
            DirectCaptureProbeResult::Failed
        }
        Err(_) => DirectCaptureProbeResult::TimedOut,
    }
}

async fn bounded_screen_recording_capturable() -> DirectCaptureProbeResult {
    run_direct_capture_probe(
        DIRECT_CAPTURE_PROBE_TIMEOUT,
        tokio::task::spawn_blocking(screen_recording_capturable),
    )
    .await
}

fn bounded_native_setup_frame() -> (
    DirectCaptureProbeResult,
    Option<crate::capture::NativePermissionSetupFrame>,
) {
    match crate::capture::capture_native_permission_setup_frame() {
        Ok(frame) => (DirectCaptureProbeResult::Ready, Some(frame)),
        Err(error) if crate::capture::native_capture_timed_out(&error) => {
            (DirectCaptureProbeResult::TimedOut, None)
        }
        Err(error) => {
            tracing::warn!(%error, "native ScreenCaptureKit setup-frame probe failed");
            (DirectCaptureProbeResult::Failed, None)
        }
    }
}

fn should_probe_direct_capture(
    should_prompt: bool,
    screen_recording: bool,
    probe_direct_capture: bool,
) -> bool {
    should_prompt && screen_recording && probe_direct_capture
}

fn should_prompt_permissions(requested: bool, host_owns_permission_ux: bool) -> bool {
    requested && !cua_driver_core::embedded_mode() && !host_owns_permission_ux
}

/// (B) Which TCC identity the booleans in this response reflect.
///
/// macOS attributes Accessibility / Screen-Recording to the *responsible
/// process* (the LaunchServices launching app), not the executable path.
/// So `check_permissions` answered by the daemon reflects:
///   - the **CuaDriver daemon** (`com.trycua.driver`) when this process is
///     its own responsible process — the real driver status.
///   - the **embedding host** otherwise. That is intentional only when the
///     host directly spawned `cua-driver serve --embedded`.
fn permission_source(
    host_owns_permission_ux: bool,
    configured_host_bundle_id: Option<&str>,
) -> serde_json::Value {
    let pid = unsafe { libc::getpid() };
    let ppid = unsafe { libc::getppid() };
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_default();
    let disclaimed = std::env::var_os(cua_driver_core::RESPONSIBILITY_DISCLAIMED_ENV).is_some();
    // Embedded mode: the driver is a child in a host app's responsibility
    // chain, so the probes already answer for the host's TCC identity.
    // This branch only ever downgrades attribution (host, never
    // driver-daemon), so the caller-controlled env var can't spoof an
    // elevated identity. `host_bundle_id` is advisory, not a trust signal.
    if host_owns_permission_ux || cua_driver_core::embedded_mode() {
        let host_bundle_id = configured_host_bundle_id
            .map(str::to_owned)
            .or_else(|| std::env::var(cua_driver_core::HOST_BUNDLE_ID_ENV).ok())
            .unwrap_or_default();
        return serde_json::json!({
            "attribution": "host",
            "host_bundle_id": host_bundle_id,
            "embedded": cua_driver_core::embedded_mode(),
            "direct_runtime": host_owns_permission_ux && !cua_driver_core::embedded_mode(),
            "pid": pid,
            "responsible_ppid": ppid,
            "executable": exe,
            "disclaim_env": disclaimed,
            "note": "Embedded mode: these booleans reflect the HOST app's TCC \
                     grant (the driver is a child in the host's responsibility \
                     chain). No separate driver grant exists or is needed. If a \
                     permission is NOT granted, the host app must request it — \
                     the driver never raises its own prompt.",
        });
    }
    // The trustworthy, non-spoofable signal is the executable path: a caller
    // can't run from inside the code-signed `CuaDriver.app` bundle without
    // controlling that install. The disclaim env var is caller-controlled, so
    // it is treated only as a corroborating signal that explains why a
    // bundle-resident daemon has `ppid != 1` (it re-exec'd itself with
    // responsibility disclaim, so launchd is no longer its parent). On its own
    // — outside the bundle — the env var must NOT grant daemon attribution, or
    // a caller could pre-set it and spoof the TCC source. Fail closed to
    // "caller" whenever the bundle signal is absent.
    let driver_bundle_id = driver_bundle_id_for_executable(&exe);
    let is_driver_daemon = driver_bundle_id.is_some() && (ppid == 1 || disclaimed);

    let (attribution, note) = if is_driver_daemon {
        (
            "driver-daemon",
            format!(
                "These booleans reflect the CuaDriver daemon's own TCC identity \
                 ({}) because this process is its own responsible process.",
                driver_bundle_id.expect("driver daemon must have a bundle id")
            ),
        )
    } else {
        (
            "caller",
            "These booleans reflect the TCC identity of the app that launched \
             this process (e.g. your terminal/IDE), NOT an installed CuaDriver \
             app bundle. A standalone check can read `true` here while the \
             driver's bundle has no grant. To grant for the driver, run \
             `cua-driver permissions grant`."
                .to_owned(),
        )
    };

    serde_json::json!({
        "attribution": attribution,
        "pid": pid,
        "responsible_ppid": ppid,
        "executable": exe,
        "disclaim_env": disclaimed,
        "bundle_id": driver_bundle_id,
        "note": note,
    })
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        // Matches Swift `CheckPermissionsTool.swift` description verbatim.
        name: "check_permissions".into(),
        description: "Report TCC permission status for Accessibility and Screen Recording. \
            By default also raises the system permission dialogs for any missing grants — \
            Apple's request APIs are no-ops when the grant is already active, so this is \
            safe to call repeatedly. Pass {\"prompt\": false} for a purely read-only \
            status check.\n\n\
            Returns: `accessibility` + `screen_recording` (booleans from the TCC \
            preflight APIs), `screen_recording_capturable` (a live ScreenCaptureKit \
            probe when `prompt` is true; null on read-only calls), \
            `direct_capture_status` (`ready`, `unavailable`, `timed_out`, `probe_failed`, \
            `blocked_by_screen_recording`, or `not_checked`), `direct_capture_error` (a structured \
            timeout/probe failure when applicable), native backend/operation/frame provenance, \
            and `source` (which TCC identity the \
            booleans reflect: the CuaDriver daemon vs the launching terminal/IDE). \
            macOS attributes grants to the responsible process, so a standalone call \
            from a terminal reports the terminal's grants, not the driver's. The \
            prompt-capable ScreenCaptureKit probe never runs when `prompt` is false. \
            Pass `probe_direct_capture:false` with `prompt:true` to register/request only \
            the two required TCC grants before separately explaining Tahoe's direct-capture \
            consent.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "boolean",
                    "description": "Raise the system permission prompts for missing grants. Default false; only a trusted host setup route may set true.",
                    "default": false,
                },
                "probe_direct_capture": {
                    "type": "boolean",
                    "description": "When prompting and Screen Recording is granted, also run the live ScreenCaptureKit probe that may raise Tahoe's direct-capture consent. Default true. Set false for a staged Accessibility/Screen Recording request.",
                }
            },
            "additionalProperties": false,
        }),
        // Not read_only because an explicit prompt=true would raise a modal
        // dialog if invoked by the trusted host helper. The public registry
        // refuses that shape before platform dispatch.
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for CheckPermissionsTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn protected_resource_ownership(
        &self,
        adapter_id: &str,
        _args: &Value,
    ) -> ProtectedResourceOwnership {
        if adapter_id == "os_permission_prompt"
            && !should_prompt_permissions(true, self.state.host_owns_permission_ux)
        {
            ProtectedResourceOwnership::DriverOwned
        } else {
            ProtectedResourceOwnership::UserOwned
        }
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        // Public calls default to read-only inspection. Only the
        // LaunchServices-hosted setup route passes prompt=true.
        // Embedded mode hard-disables prompting regardless of the arg (the
        // host owns the grant flow). This and the startup gate are the only
        // `request_*` call sites, so both being gated makes prompts
        // unreachable when embedded.
        let should_prompt = should_prompt_permissions(
            args.bool_or("prompt", false),
            self.state.host_owns_permission_ux,
        );
        let probe_direct_capture = args.bool_or("probe_direct_capture", true);
        if should_prompt {
            let _ = request_accessibility();
            let _ = request_screen_recording();
        }
        let accessibility = accessibility_granted();
        let screen_recording = screen_recording_granted();
        // (A) Authoritative live probe — see `screen_recording_capturable`.
        // SCShareableContent::get() can itself raise Tahoe's separate
        // private-window-picker bypass consent. A status/read-only call must
        // therefore never execute it. Only the private LaunchServices setup
        // constructor selects the stronger SCScreenshotManager frame probe;
        // public tool instances remain enumeration-only.
        let (screen_recording_capturable, direct_capture_status, direct_capture_error, provenance) =
            if !should_prompt {
                (
                    None,
                    "not_checked",
                    None,
                    DirectCaptureProvenance::default(),
                )
            } else if !screen_recording {
                (
                    None,
                    "blocked_by_screen_recording",
                    None,
                    DirectCaptureProvenance::default(),
                )
            } else if should_probe_direct_capture(
                should_prompt,
                screen_recording,
                probe_direct_capture,
            ) {
                let (result, frame) = if self.native_setup_frame_probe {
                    bounded_native_setup_frame()
                } else {
                    (bounded_screen_recording_capturable().await, None)
                };
                let (capturable, status, error) = result.response_fields();
                (
                    capturable,
                    status,
                    error,
                    if self.native_setup_frame_probe {
                        DirectCaptureProvenance::native_attempt(frame)
                    } else {
                        DirectCaptureProvenance::default()
                    },
                )
            } else {
                (
                    None,
                    "not_checked",
                    None,
                    DirectCaptureProvenance::default(),
                )
            };
        // (B) Which identity the booleans above belong to.
        let source = permission_source(
            self.state.host_owns_permission_ux,
            self.state.host_bundle_id.as_deref(),
        );
        let is_caller = source.get("attribution").and_then(|v| v.as_str()) == Some("caller");

        // Text format mirrors Swift 1:1:
        //   "✅ Accessibility: granted.\n✅ Screen Recording: granted."
        let ax_prefix = if accessibility { "✅" } else { "❌" };
        let sr_prefix = if screen_recording { "✅" } else { "❌" };
        let ax_state = if accessibility {
            "granted"
        } else {
            "NOT granted"
        };
        let sr_state = if screen_recording {
            "granted"
        } else {
            "NOT granted"
        };
        let mut summary = format!(
            "{ax_prefix} Accessibility: {ax_state}.\n{sr_prefix} Screen Recording: {sr_state}."
        );
        // Flag a preflight/probe disagreement (the false-positive tell).
        if screen_recording_capturable == Some(false) {
            summary.push_str(
                "\n⚠️  Screen Recording reads granted but a live capture probe failed — \
                 the grant likely belongs to a different process, not this one.",
            );
        } else if direct_capture_status == "timed_out" {
            summary.push_str(
                "\n⚠️  The direct ScreenCaptureKit readiness probe timed out; the permission \
                 check returned without waiting indefinitely.",
            );
        } else if direct_capture_status == "probe_failed" {
            summary.push_str(
                "\n⚠️  The direct ScreenCaptureKit readiness probe failed; see \
                 direct_capture_error for the bounded failure code.",
            );
        } else if screen_recording_capturable.is_none() && (!should_prompt || !probe_direct_capture)
        {
            summary.push_str(
                "\nℹ️  Direct ScreenCaptureKit readiness was not probed because this is a \
                 staged or read-only check. Run `cua-driver permissions grant` to request \
                 and verify direct capture explicitly.",
            );
        }
        // Make the attribution explicit when answering for a host or caller
        // (not the daemon).
        if source.get("attribution").and_then(|v| v.as_str()) == Some("host") {
            summary.push_str(
                "\nℹ️  Embedded mode: status reflects the HOST app's TCC grant. \
                 If a permission is missing, the host must request it — the \
                 driver will not prompt.",
            );
        }
        if is_caller {
            summary.push_str(
                "\nℹ️  Status reflects the launching app's TCC identity, not the CuaDriver \
                 daemon (com.trycua.driver). See `source` for details.",
            );
        }

        ToolResult::text(summary).with_structured(serde_json::json!({
            "accessibility":               accessibility,
            "screen_recording":            screen_recording,
            "screen_recording_capturable": screen_recording_capturable,
            "direct_capture_status":        direct_capture_status,
            "direct_capture_error":         direct_capture_error,
            "direct_capture_backend":       provenance.backend,
            "direct_capture_operation":     provenance.operation,
            "direct_capture_frame_captured": provenance.frame_captured,
            "direct_capture_frame_width":   provenance.frame_width,
            "direct_capture_frame_height":  provenance.frame_height,
            "direct_capture_frame_byte_count": provenance.frame_byte_count,
            "direct_capture_fallback_used": provenance.fallback_used,
            "source":                      source,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::permissions::test_env_lock()
    }

    /// Set/remove `var`, returning the original for restore. Callers must
    /// hold `env_lock()`.
    fn swap_env(var: &str, value: Option<&str>) -> Option<std::ffi::OsString> {
        let original = std::env::var_os(var);
        match value {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
        original
    }

    fn restore_env(var: &str, original: Option<std::ffi::OsString>) {
        match original {
            Some(value) => std::env::set_var(var, value),
            None => std::env::remove_var(var),
        }
    }

    #[test]
    fn recognizes_release_and_local_driver_bundles() {
        assert_eq!(
            driver_bundle_id_for_executable(
                "/Applications/CuaDriver.app/Contents/MacOS/cua-driver"
            ),
            Some("com.trycua.driver")
        );
        assert_eq!(
            driver_bundle_id_for_executable(
                "/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local"
            ),
            Some("com.trycua.driver.local")
        );
        assert_eq!(
            driver_bundle_id_for_executable("/Users/test/.local/bin/cua-driver-local"),
            None
        );
    }

    #[test]
    fn read_only_checks_never_run_the_prompt_capable_direct_capture_probe() {
        assert!(!should_probe_direct_capture(false, false, true));
        assert!(!should_probe_direct_capture(false, true, true));
        assert!(!should_probe_direct_capture(true, false, true));
        assert!(should_probe_direct_capture(true, true, true));
    }

    #[test]
    fn direct_host_runtime_cannot_raise_permission_prompts() {
        let _guard = env_lock();
        let original = swap_env(cua_driver_core::EMBEDDED_ENV, None);
        assert!(
            should_prompt_permissions(true, false),
            "standalone Cua-owned runtime retains its explicit prompt path"
        );
        assert!(
            !should_prompt_permissions(true, true),
            "direct host-owned runtime must force read-only permission checks"
        );
        restore_env(cua_driver_core::EMBEDDED_ENV, original);
    }

    #[test]
    fn direct_runtime_reports_host_attribution() {
        let _guard = env_lock();
        let original = swap_env(cua_driver_core::EMBEDDED_ENV, None);
        let source = permission_source(true, None);
        assert_eq!(
            source.get("attribution").and_then(|value| value.as_str()),
            Some("host")
        );
        assert_eq!(
            source
                .get("direct_runtime")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
        restore_env(cua_driver_core::EMBEDDED_ENV, original);
    }

    #[test]
    fn immutable_runtime_host_label_wins_over_process_environment() {
        let _guard = env_lock();
        let original_host = swap_env(
            cua_driver_core::HOST_BUNDLE_ID_ENV,
            Some("com.example.stale"),
        );
        let source = permission_source(true, Some("com.example.runtime"));
        assert_eq!(source["host_bundle_id"], "com.example.runtime");
        restore_env(cua_driver_core::HOST_BUNDLE_ID_ENV, original_host);
    }

    #[test]
    fn staged_prompt_never_runs_the_direct_capture_probe() {
        assert!(!should_probe_direct_capture(true, false, false));
        assert!(!should_probe_direct_capture(true, true, false));
    }

    #[tokio::test]
    async fn direct_capture_probe_returns_before_a_hung_probe() {
        let result = run_direct_capture_probe(
            std::time::Duration::from_millis(10),
            std::future::pending::<Result<bool, tokio::task::JoinError>>(),
        )
        .await;

        assert_eq!(result, DirectCaptureProbeResult::TimedOut);
        let (capturable, status, error) = result.response_fields();
        assert_eq!(capturable, None);
        assert_eq!(status, "timed_out");
        assert_eq!(error.unwrap()["code"], "direct_capture_probe_timed_out");
    }

    #[tokio::test]
    async fn direct_capture_probe_preserves_successful_results() {
        let ready = run_direct_capture_probe(
            std::time::Duration::from_secs(1),
            std::future::ready(Ok(true)),
        )
        .await;
        let unavailable = run_direct_capture_probe(
            std::time::Duration::from_secs(1),
            std::future::ready(Ok(false)),
        )
        .await;

        assert_eq!(ready, DirectCaptureProbeResult::Ready);
        assert_eq!(unavailable, DirectCaptureProbeResult::Unavailable);
    }

    #[test]
    fn capability_enumeration_preserves_legacy_ready_status() {
        let (_, status, _) = DirectCaptureProbeResult::Ready.response_fields();
        assert_eq!(status, "ready", "keep the existing public status contract");
    }

    #[test]
    fn trusted_native_frame_has_explicit_no_fallback_provenance() {
        let provenance = DirectCaptureProvenance::native_attempt(Some(
            crate::capture::NativePermissionSetupFrame {
                width: 2,
                height: 2,
                rgba_byte_count: 16,
            },
        ));

        assert_eq!(provenance.backend, Some("screencapturekit"));
        assert_eq!(
            provenance.operation,
            Some("screenshot_manager_display_frame")
        );
        assert_eq!(provenance.frame_captured, Some(true));
        assert_eq!(provenance.frame_width, Some(2));
        assert_eq!(provenance.frame_height, Some(2));
        assert_eq!(provenance.frame_byte_count, Some(16));
        assert_eq!(provenance.fallback_used, Some(false));
    }

    #[test]
    fn only_trusted_permission_setup_selects_the_native_frame_probe() {
        let state = Arc::new(ToolState::new(false, false, None));
        let public = CheckPermissionsTool::new(Arc::clone(&state));
        let trusted = CheckPermissionsTool::for_trusted_permission_setup(state);

        assert_eq!(
            public.native_setup_frame_probe, false,
            "registered tool keeps the non-frame legacy probe"
        );
        assert_eq!(
            trusted.native_setup_frame_probe, true,
            "only the private permission helper captures a setup frame"
        );
    }

    #[test]
    fn disclaim_env_var_alone_does_not_grant_daemon_attribution() {
        // The disclaim env var is caller-controlled, so on its own it must not
        // make `check_permissions` claim the booleans reflect the daemon's TCC
        // identity. Daemon attribution additionally requires the binary to live
        // inside the code-signed `CuaDriver.app` bundle — the test runner does
        // not, so even with the env var present we must fail closed to "caller".
        let _guard = env_lock();
        let name = cua_driver_core::RESPONSIBILITY_DISCLAIMED_ENV;
        let original = swap_env(name, Some("1"));
        let embedded = swap_env(cua_driver_core::EMBEDDED_ENV, None);

        let source = permission_source(false, None);
        assert_eq!(
            source.get("attribution").and_then(|v| v.as_str()),
            Some("caller"),
            "env-var presence alone must not yield daemon attribution"
        );

        restore_env(cua_driver_core::EMBEDDED_ENV, embedded);
        restore_env(name, original);
    }

    #[test]
    fn embedded_mode_reports_host_attribution() {
        let _guard = env_lock();
        let embedded = swap_env(cua_driver_core::EMBEDDED_ENV, Some("1"));
        let host = swap_env(
            cua_driver_core::HOST_BUNDLE_ID_ENV,
            Some("com.example.host"),
        );

        let source = permission_source(false, None);
        assert_eq!(
            source.get("attribution").and_then(|v| v.as_str()),
            Some("host"),
        );
        assert_eq!(
            source.get("host_bundle_id").and_then(|v| v.as_str()),
            Some("com.example.host"),
        );
        assert_eq!(source.get("embedded").and_then(|v| v.as_bool()), Some(true));

        restore_env(cua_driver_core::HOST_BUNDLE_ID_ENV, host);
        restore_env(cua_driver_core::EMBEDDED_ENV, embedded);
    }

    #[test]
    fn embedded_plus_disclaim_env_never_yields_daemon_attribution() {
        // Both caller-controlled env vars together must still not produce
        // "driver-daemon" — embedded mode may only DOWNGRADE attribution.
        let _guard = env_lock();
        let embedded = swap_env(cua_driver_core::EMBEDDED_ENV, Some("1"));
        let disclaim = swap_env(cua_driver_core::RESPONSIBILITY_DISCLAIMED_ENV, Some("1"));

        let source = permission_source(false, None);
        assert_eq!(
            source.get("attribution").and_then(|v| v.as_str()),
            Some("host"),
        );

        restore_env(cua_driver_core::RESPONSIBILITY_DISCLAIMED_ENV, disclaim);
        restore_env(cua_driver_core::EMBEDDED_ENV, embedded);
    }

    #[test]
    fn embedded_env_requires_exact_value_one() {
        let _guard = env_lock();
        let embedded = swap_env(cua_driver_core::EMBEDDED_ENV, Some("true"));
        let source = permission_source(false, None);
        assert_ne!(
            source.get("attribution").and_then(|v| v.as_str()),
            Some("host"),
            "only CUA_DRIVER_EMBEDDED=1 may enable embedded mode"
        );
        restore_env(cua_driver_core::EMBEDDED_ENV, embedded);
    }
}
