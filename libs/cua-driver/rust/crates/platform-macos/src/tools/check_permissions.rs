use async_trait::async_trait;
use cua_driver_core::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

use crate::permissions::status::{
    accessibility_granted, request_accessibility, request_screen_recording,
    request_system_events, screen_recording_granted, system_events_granted,
};

pub struct CheckPermissionsTool;

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
/// macOS attributes Accessibility / Screen Recording / Automation to the *responsible
/// process* (the LaunchServices launching app), not the executable path.
/// So `check_permissions` answered in-process reflects:
///   - the **CuaDriver daemon** (`com.trycua.driver`) when this process is
///     its own responsible process — the real driver status.
///   - the **calling app** otherwise — e.g. the terminal/IDE that spawned
///     `cua-driver call …`. That grant is NOT the driver's, which is why a
///     standalone check can read `true` while `tccutil … com.trycua.driver`
///     reports no record.
fn permission_source() -> serde_json::Value {
    let pid = unsafe { libc::getpid() };
    let ppid = unsafe { libc::getppid() };
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_default();
    // The trustworthy, non-spoofable signal is the executable path: a caller
    // can't run from inside the code-signed `CuaDriver.app` bundle without
    // controlling that install. The disclaim env var is caller-controlled, so
    // it is treated only as a corroborating signal that explains why a
    // bundle-resident daemon has `ppid != 1` (it re-exec'd itself with
    // responsibility disclaim, so launchd is no longer its parent). On its own
    // — outside the bundle — the env var must NOT grant daemon attribution, or
    // a caller could pre-set it and spoof the TCC source. Fail closed to
    // "caller" whenever the bundle signal is absent.
    let inside_bundle = exe.contains("/CuaDriver.app/Contents/MacOS/");
    let disclaimed =
        std::env::var_os(cua_driver_core::RESPONSIBILITY_DISCLAIMED_ENV).is_some();
    let is_driver_daemon = inside_bundle && (ppid == 1 || disclaimed);

    let (attribution, note) = if is_driver_daemon {
        (
            "driver-daemon",
            "These booleans reflect the CuaDriver daemon's own TCC identity \
             (com.trycua.driver) because this process is its own responsible \
             process.",
        )
    } else {
        (
            "caller",
            "These booleans reflect the TCC identity of the app that launched \
             this process (e.g. your terminal/IDE), NOT the CuaDriver daemon \
             (com.trycua.driver). A standalone check can read `true` here while \
             `tccutil … com.trycua.driver` reports no record. To grant for the \
             driver, run `cua-driver permissions grant`.",
        )
    };

    serde_json::json!({
        "attribution": attribution,
        "pid": pid,
        "responsible_ppid": ppid,
        "executable": exe,
        "note": note,
    })
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        // Matches Swift `CheckPermissionsTool.swift` description verbatim.
        name: "check_permissions".into(),
        description: "Report TCC permission status for Accessibility, Screen Recording, \
            and Automation access to System Events. \
            By default also raises the system permission dialogs for any missing grants — \
            Apple's request APIs are no-ops when the grant is already active, so this is \
            safe to call repeatedly. Pass {\"prompt\": false} for a purely read-only \
            status check.\n\n\
            Returns: `accessibility` + `screen_recording` + `system_events` \
            (booleans from the TCC preflight APIs), `screen_recording_capturable` \
            (a live ScreenCaptureKit \
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
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        // Default to prompting — same default + rationale as Swift.
        let should_prompt = args.bool_or("prompt", true);
        if should_prompt {
            let _ = request_accessibility();
            let _ = request_screen_recording();
            let _ = request_system_events();
        }
        let accessibility = accessibility_granted();
        let screen_recording = screen_recording_granted();
        let system_events = system_events_granted();
        // (A) Authoritative live probe — see `screen_recording_capturable`.
        let screen_recording_capturable = screen_recording_capturable();
        // (B) Which identity the booleans above belong to.
        let source = permission_source();
        let is_caller = source.get("attribution").and_then(|v| v.as_str()) == Some("caller");

        // Text format mirrors Swift 1:1:
        //   "✅ Accessibility: granted.\n✅ Screen Recording: granted."
        let ax_prefix  = if accessibility   { "✅" } else { "❌" };
        let sr_prefix  = if screen_recording { "✅" } else { "❌" };
        let se_prefix  = if system_events { "✅" } else { "❌" };
        let ax_state   = if accessibility   { "granted" } else { "NOT granted" };
        let sr_state   = if screen_recording { "granted" } else { "NOT granted" };
        let se_state   = if system_events { "granted" } else { "NOT granted" };
        let mut summary = format!(
            "{ax_prefix} Accessibility: {ax_state}.\n\
             {sr_prefix} Screen Recording: {sr_state}.\n\
             {se_prefix} Automation (System Events): {se_state}."
        );
        // Flag a preflight/probe disagreement (the false-positive tell).
        if screen_recording && !screen_recording_capturable {
            summary.push_str(
                "\n⚠️  Screen Recording reads granted but a live capture probe failed — \
                 the grant likely belongs to a different process, not this one.",
            );
        }
        // Make the attribution explicit when answering for the caller (not the daemon).
        if is_caller {
            summary.push_str(
                "\nℹ️  Status reflects the launching app's TCC identity, not the CuaDriver \
                 daemon (com.trycua.driver). See `source` for details.",
            );
        }

        ToolResult::text(summary)
            .with_structured(serde_json::json!({
                "accessibility":               accessibility,
                "screen_recording":            screen_recording,
                "system_events":               system_events,
                "screen_recording_capturable": screen_recording_capturable,
                "source":                      source,
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disclaim_env_var_alone_does_not_grant_daemon_attribution() {
        // The disclaim env var is caller-controlled, so on its own it must not
        // make `check_permissions` claim the booleans reflect the daemon's TCC
        // identity. Daemon attribution additionally requires the binary to live
        // inside the code-signed `CuaDriver.app` bundle — the test runner does
        // not, so even with the env var present we must fail closed to "caller".
        let name = cua_driver_core::RESPONSIBILITY_DISCLAIMED_ENV;
        let original = std::env::var_os(name);

        std::env::set_var(name, "1");
        let source = permission_source();
        assert_eq!(
            source.get("attribution").and_then(|v| v.as_str()),
            Some("caller"),
            "env-var presence alone must not yield daemon attribution"
        );

        match original {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
}
