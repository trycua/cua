use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

use crate::permissions::status::{
    accessibility_granted, request_accessibility, request_screen_recording,
    screen_recording_granted,
};

pub struct CheckPermissionsTool;

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

/// (B) Which TCC identity the booleans in this response reflect.
///
/// macOS attributes Accessibility / Screen-Recording to the *responsible
/// process* (the LaunchServices launching app), not the executable path.
/// So `check_permissions` answered by the daemon reflects:
///   - the **CuaDriver daemon** (`com.trycua.driver`) when this process is
///     its own responsible process — the real driver status.
///   - the **embedding host** otherwise. That is intentional only when the
///     host directly spawned `cua-driver serve --embedded`.
fn permission_source() -> serde_json::Value {
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
    if cua_driver_core::embedded_mode() {
        let host_bundle_id = std::env::var(cua_driver_core::HOST_BUNDLE_ID_ENV).unwrap_or_default();
        return serde_json::json!({
            "attribution": "host",
            "host_bundle_id": host_bundle_id,
            "embedded": true,
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
            probe — if it disagrees with `screen_recording`, the preflight grant \
            belongs to a different process), and `source` (which TCC identity the \
            booleans reflect: the CuaDriver daemon vs the launching terminal/IDE). \
            macOS attributes grants to the responsible process, so a standalone call \
            from a terminal reports the terminal's grants, not the driver's.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "boolean",
                    "description": "Raise the system permission prompts for missing grants. Default true.",
                }
            },
            "additionalProperties": false,
        }),
        // Not read_only because the default path may raise a modal dialog
        // (mirrors Swift annotation `readOnlyHint: false`).
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

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        // Default to prompting — same default + rationale as Swift.
        // Embedded mode hard-disables prompting regardless of the arg (the
        // host owns the grant flow). This and the startup gate are the only
        // `request_*` call sites, so both being gated makes prompts
        // unreachable when embedded.
        let should_prompt = args.bool_or("prompt", true) && !cua_driver_core::embedded_mode();
        if should_prompt {
            let _ = request_accessibility();
            let _ = request_screen_recording();
        }
        let accessibility = accessibility_granted();
        let screen_recording = screen_recording_granted();
        // (A) Authoritative live probe — see `screen_recording_capturable`.
        let screen_recording_capturable = screen_recording_capturable();
        // (B) Which identity the booleans above belong to.
        let source = permission_source();
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
        if screen_recording && !screen_recording_capturable {
            summary.push_str(
                "\n⚠️  Screen Recording reads granted but a live capture probe failed — \
                 the grant likely belongs to a different process, not this one.",
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

        let source = permission_source();
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

        let source = permission_source();
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

        let source = permission_source();
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
        let source = permission_source();
        assert_ne!(
            source.get("attribution").and_then(|v| v.as_str()),
            Some("host"),
            "only CUA_DRIVER_EMBEDDED=1 may enable embedded mode"
        );
        restore_env(cua_driver_core::EMBEDDED_ENV, embedded);
    }
}
