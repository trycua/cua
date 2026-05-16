//! First-launch permissions gate — CLI flow.
//!
//! Rust port of Swift's `PermissionsGate` (SwiftUI panel + polling).  The
//! Rust port intentionally drops the SwiftUI window and reimplements the
//! flow as a terminal-only experience:
//!
//!   1. Inspect TCC state on `serve` startup.
//!   2. If any required grant is missing, print a clear explanation
//!      (which grant, why cua-driver needs it, what to do next).
//!   3. Open the relevant `System Settings` pane(s) via the
//!      `x-apple.systempreferences:` URL scheme.
//!   4. Poll TCC state every second.  Emit a "still waiting" line every
//!      5 seconds so the user knows the daemon is still alive and what
//!      it is waiting for.
//!   5. As soon as everything flips green, print a confirmation and
//!      return — `serve` proceeds normally.
//!
//! Opt-out: `--no-permissions-gate` flag or
//! `CUA_DRIVER_RS_PERMISSIONS_GATE` set to `0` / `false` / `no` / `off`
//! (case-insensitive) skips the entire flow.  Intended for CI / headless
//! automation where blocking on user input would deadlock the runner.
//!
//! Why no GUI window: the Swift gate uses AppKit + SwiftUI which would
//! require a full overlay + NSApplication run loop just to display a
//! dialog.  cua-driver-rs already has a separate AppKit thread for the
//! cursor overlay, and grafting another window onto it is a recipe for
//! main-thread deadlocks.  A terminal-driven flow is uglier but reliable
//! and works headless (with the opt-out flag), which is exactly the
//! audience for the Rust port.
//!
//! A future enhancement could open a native `NSAlert` via objc2 for a
//! more polished look — left as a follow-up; CLI is the MVP.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::permissions::status::{
    current_status, request_accessibility, request_screen_recording, PermissionsStatus,
};

/// Which TCC grant is missing.  Each variant maps 1:1 to a System Settings
/// pane URL via [`MissingPermission::settings_url`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingPermission {
    Accessibility,
    ScreenRecording,
}

impl MissingPermission {
    /// Short human-readable label used in CLI output.
    pub fn label(self) -> &'static str {
        match self {
            Self::Accessibility => "Accessibility",
            Self::ScreenRecording => "Screen Recording",
        }
    }

    /// One-line rationale shown in the missing-permission listing.  Text
    /// adapted from the Swift gate's SwiftUI subtitle copy.
    pub fn rationale(self) -> &'static str {
        match self {
            Self::Accessibility =>
                "lets cua-driver read the accessibility tree of running apps and \
                 send clicks / keystrokes via AX RPC.",
            Self::ScreenRecording =>
                "lets cua-driver capture per-window screenshots so agents can see \
                 the current UI state alongside the tree.",
        }
    }

    /// `x-apple.systempreferences:` URL that deep-links into the matching
    /// Privacy pane.  Same strings the Swift gate uses.
    pub fn settings_url(self) -> &'static str {
        match self {
            Self::Accessibility =>
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            Self::ScreenRecording =>
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
        }
    }
}

/// All missing required permissions, derived from a [`PermissionsStatus`]
/// snapshot.  Returns an empty vec when everything is granted.
pub fn missing_from_status(status: PermissionsStatus) -> Vec<MissingPermission> {
    let mut out = Vec::new();
    if !status.accessibility {
        out.push(MissingPermission::Accessibility);
    }
    if !status.screen_recording {
        out.push(MissingPermission::ScreenRecording);
    }
    out
}

/// Inspect live TCC state and return whatever is missing.  Convenience
/// wrapper around `missing_from_status(current_status())`.
pub fn check_required_permissions() -> Vec<MissingPermission> {
    missing_from_status(current_status())
}

/// Open the System Settings pane for a single permission via `open(1)`.
///
/// macOS routes `x-apple.systempreferences:` URLs through `System Settings`
/// automatically — same mechanism the Swift gate uses via `NSWorkspace.open`.
pub fn open_system_settings_for(permission: MissingPermission) -> Result<()> {
    let status = std::process::Command::new("open")
        .arg(permission.settings_url())
        .status()?;
    if !status.success() {
        anyhow::bail!(
            "`open {}` exited with status {:?}",
            permission.settings_url(),
            status.code()
        );
    }
    Ok(())
}

/// Knobs for [`run_if_needed`].  Defaults are tuned for an interactive
/// `serve` startup; CI callers should set `opt_out = true` (either via
/// the `--no-permissions-gate` flag or the env-var honored by
/// [`GateOpts::from_env_and_flag`]).
#[derive(Debug, Clone)]
pub struct GateOpts {
    /// When `true` the gate is a no-op even if permissions are missing.
    pub opt_out: bool,
    /// Cap the polling phase so a forgotten daemon does not hang
    /// forever.  `None` means "wait indefinitely" — matches Swift's
    /// SwiftUI panel which has no built-in timeout.  Default: 10 min.
    pub deadline: Option<Duration>,
    /// How often to re-check TCC state.  Default: 1s — same cadence as
    /// the Swift gate's `Timer.scheduledTimer(withTimeInterval: 1.0)`.
    pub poll_interval: Duration,
    /// How often to print a "still waiting for X" status line.  Default:
    /// 5s — frequent enough to reassure the user the daemon is alive,
    /// rare enough not to spam the terminal.
    pub status_interval: Duration,
    /// When `true` and a required permission is missing, also raise the
    /// macOS TCC prompts (`AXIsProcessTrustedWithOptions` /
    /// `CGRequestScreenCaptureAccess`).  Helpful on first launch when
    /// the process has never asked before.  Default: true.
    pub also_raise_prompts: bool,
    /// When `true` (default), `open` the System Settings pane for the
    /// missing permission(s).  Set false to suppress the auto-open in
    /// tests or scripted scenarios.
    pub open_settings: bool,
}

impl Default for GateOpts {
    fn default() -> Self {
        Self {
            opt_out: false,
            deadline: Some(Duration::from_secs(10 * 60)),
            poll_interval: Duration::from_secs(1),
            status_interval: Duration::from_secs(5),
            also_raise_prompts: true,
            open_settings: true,
        }
    }
}

impl GateOpts {
    /// Construct from the standard env-var
    /// (`CUA_DRIVER_RS_PERMISSIONS_GATE` set to `0` / `false` / `no` /
    /// `off`, case-insensitive, disables the gate) and an explicit
    /// `--no-permissions-gate` flag.  Either signal is sufficient to opt
    /// out.
    pub fn from_env_and_flag(no_gate_flag: bool) -> Self {
        // Match the standard list of "off" sentinels case-insensitively so
        // CI scripts can use any of `0`, `false`, `no`, `off`, `FALSE`,
        // `Off`, etc. without surprises.  Anything not in this set leaves
        // the gate active — fail-safe default for first-launch UX.
        let env_disabled = std::env::var("CUA_DRIVER_RS_PERMISSIONS_GATE")
            .ok()
            .map(|v| {
                let lower = v.to_ascii_lowercase();
                matches!(lower.as_str(), "0" | "false" | "no" | "off")
            })
            .unwrap_or(false);
        Self {
            opt_out: no_gate_flag || env_disabled,
            ..Self::default()
        }
    }
}

/// Run the gate if needed.  When called and the process already has both
/// grants, this returns immediately without printing anything — the
/// `serve` happy path is unaffected.
///
/// When grants are missing and `opt_out` is false:
///   - Prints the missing-permissions banner.
///   - Optionally raises the system TCC prompts (`also_raise_prompts`).
///   - Opens the System Settings pane(s) for the user.
///   - Polls TCC every `poll_interval` and re-emits a status line every
///     `status_interval` until everything is green or `deadline` elapses.
///
/// Returns `Ok(())` on success (all green or opt-out).  Returns
/// `Err` only if the deadline elapsed without all permissions granted —
/// callers may choose to continue anyway and let individual tools fail
/// with their existing error messages, mirroring Swift's "user closes
/// the panel" path.
pub fn run_if_needed(opts: GateOpts) -> Result<()> {
    if opts.opt_out {
        tracing::debug!("permissions gate skipped (opt_out=true)");
        return Ok(());
    }

    let initial = current_status();
    if initial.all_granted() {
        // Fast path: everything already green.  No banner, no polling —
        // the user sees nothing different from before this gate existed.
        return Ok(());
    }

    let missing = missing_from_status(initial);
    print_banner(&missing, opts.open_settings);

    if opts.also_raise_prompts {
        // These are no-ops when the grant is already active and are the
        // documented way to fire the first-launch system dialogs.  Match
        // the behaviour of the `check_permissions` MCP tool's default path.
        if missing.contains(&MissingPermission::Accessibility) {
            let _ = request_accessibility();
        }
        if missing.contains(&MissingPermission::ScreenRecording) {
            let _ = request_screen_recording();
        }
    }

    if opts.open_settings {
        // Open *both* missing panes up front.  System Settings collapses
        // duplicate-open requests to a single navigation, so this isn't
        // disruptive even when only one grant is needed.
        for m in &missing {
            if let Err(e) = open_system_settings_for(*m) {
                eprintln!("  (could not auto-open Settings for {}: {e})", m.label());
            }
        }
    }

    wait_for_grants(&opts)
}

/// Block until all required permissions are granted or the deadline
/// elapses.  Emits a status line every `opts.status_interval` while
/// waiting so the user has feedback.
///
/// Exposed separately from [`run_if_needed`] for callers that want to
/// drive the wait phase manually (e.g. tests that pre-skip the banner).
pub fn wait_for_grants(opts: &GateOpts) -> Result<()> {
    let start = Instant::now();
    let mut last_status_print = start;
    let mut last_missing: Vec<MissingPermission> = check_required_permissions();

    loop {
        if last_missing.is_empty() {
            println!("[cua-driver] permissions granted — continuing startup");
            let _ = std::io::stdout().flush();
            return Ok(());
        }

        if let Some(deadline) = opts.deadline {
            if start.elapsed() >= deadline {
                anyhow::bail!(
                    "permissions gate timed out after {:?} — still missing: {}",
                    deadline,
                    fmt_missing(&last_missing)
                );
            }
        }

        std::thread::sleep(opts.poll_interval);

        let new_missing = check_required_permissions();
        if new_missing != last_missing {
            // State changed (red→green, or order-flipped).  Re-emit so the
            // user sees progress without waiting for the next status tick.
            if !new_missing.is_empty() {
                println!(
                    "[cua-driver] progress — still waiting on: {}",
                    fmt_missing(&new_missing)
                );
                let _ = std::io::stdout().flush();
            }
            last_status_print = Instant::now();
            last_missing = new_missing;
            continue;
        }

        if last_status_print.elapsed() >= opts.status_interval {
            println!(
                "[cua-driver] still waiting on: {} \
                 (open System Settings → Privacy & Security to grant)",
                fmt_missing(&last_missing)
            );
            let _ = std::io::stdout().flush();
            last_status_print = Instant::now();
        }
    }
}

fn print_banner(missing: &[MissingPermission], open_settings: bool) {
    println!();
    println!("──────────────────────────────────────────────────────────────");
    println!(" cua-driver needs your permission before `serve` can start");
    println!("──────────────────────────────────────────────────────────────");
    println!();
    println!(" Missing TCC grant(s) for this process:");
    for m in missing {
        println!("   • {}", m.label());
        println!("       {}", m.rationale());
    }
    println!();
    if open_settings {
        println!(" Opening System Settings → Privacy & Security now.");
    } else {
        // open_settings was suppressed by the caller — print the manual
        // command(s) instead so the user still knows where to go.  Listing
        // each pane keeps copy-paste working when only one grant is needed.
        println!(" Open System Settings → Privacy & Security manually, e.g.:");
        for m in missing {
            println!("   open \"{}\"   # {}", m.settings_url(), m.label());
        }
    }
    println!(" Grant each item, then this prompt will auto-continue.");
    println!();
    println!(" Skip this gate (CI / headless): re-run with");
    println!("   cua-driver serve --no-permissions-gate");
    println!(" or set CUA_DRIVER_RS_PERMISSIONS_GATE to 0/false/no/off");
    println!(" (case-insensitive) in the environment.");
    println!();
    let _ = std::io::stdout().flush();
}

fn fmt_missing(missing: &[MissingPermission]) -> String {
    missing
        .iter()
        .map(|m| m.label())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// Serializes every test that mutates `CUA_DRIVER_RS_PERMISSIONS_GATE`.
    /// `cargo test` runs tests in parallel by default and `std::env::set_var`
    /// / `remove_var` touch a process-global table — without this lock the
    /// env-var tests race and produce flaky failures.
    static TEST_ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        // `lock()` can only fail if a previous holder panicked.  Recover the
        // guard and keep going — the env var will be re-set/cleared by this
        // test anyway, so a poisoned mutex carries no stale invariant.
        TEST_ENV_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn opt_out_short_circuits_run_if_needed() {
        // With opt_out=true the gate must return Ok(()) without touching
        // TCC state, opening Settings, or sleeping.  We can't easily assert
        // "didn't sleep" without a clock, but we can assert that an
        // unrealistically short deadline still produces Ok — which proves
        // the wait loop was not entered.
        let opts = GateOpts {
            opt_out: true,
            deadline: Some(Duration::from_nanos(1)),
            poll_interval: Duration::from_secs(60),
            status_interval: Duration::from_secs(60),
            also_raise_prompts: false,
            open_settings: false,
        };
        assert!(run_if_needed(opts).is_ok());
    }

    #[test]
    fn env_var_disables_gate() {
        let _guard = env_lock();
        std::env::set_var("CUA_DRIVER_RS_PERMISSIONS_GATE", "0");
        let opts = GateOpts::from_env_and_flag(false);
        assert!(opts.opt_out, "env=0 must opt out");
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_GATE");
    }

    #[test]
    fn flag_disables_gate() {
        let _guard = env_lock();
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_GATE");
        let opts = GateOpts::from_env_and_flag(true);
        assert!(opts.opt_out, "--no-permissions-gate must opt out");
    }

    #[test]
    fn neither_flag_nor_env_does_not_opt_out() {
        let _guard = env_lock();
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_GATE");
        let opts = GateOpts::from_env_and_flag(false);
        assert!(!opts.opt_out);
    }

    #[test]
    fn env_var_truthy_values_do_not_opt_out() {
        let _guard = env_lock();
        // Only the explicit "off" sentinels disable the gate.  Anything
        // else (including empty string or unknown garbage) leaves the gate
        // active — fail-safe default for first-launch UX.
        for v in &["1", "true", "yes", "on", "garbage", ""] {
            std::env::set_var("CUA_DRIVER_RS_PERMISSIONS_GATE", v);
            let opts = GateOpts::from_env_and_flag(false);
            assert!(
                !opts.opt_out,
                "env={v:?} must not opt out (only 0/false/no/off do)"
            );
        }
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_GATE");
    }

    #[test]
    fn env_var_off_sentinels_are_case_insensitive() {
        let _guard = env_lock();
        // Every documented off-sentinel must opt out regardless of case so
        // CI scripts can use whatever convention they prefer.
        for v in &[
            "0", "false", "FALSE", "False", "no", "NO", "No", "off", "OFF", "Off", "TrUe",
        ] {
            std::env::set_var("CUA_DRIVER_RS_PERMISSIONS_GATE", v);
            let opts = GateOpts::from_env_and_flag(false);
            // "TrUe" is in the list intentionally — it must NOT opt out
            // (it's not in the off-sentinel set), so split the assertion.
            let expected_opt_out =
                matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off");
            assert_eq!(
                opts.opt_out, expected_opt_out,
                "env={v:?} opt_out mismatch (expected {expected_opt_out})"
            );
        }
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_GATE");
    }

    #[test]
    fn missing_from_status_orders_accessibility_first() {
        let neither = PermissionsStatus {
            accessibility: false,
            screen_recording: false,
        };
        assert_eq!(
            missing_from_status(neither),
            vec![
                MissingPermission::Accessibility,
                MissingPermission::ScreenRecording
            ]
        );

        let only_sr = PermissionsStatus {
            accessibility: false,
            screen_recording: true,
        };
        assert_eq!(
            missing_from_status(only_sr),
            vec![MissingPermission::Accessibility]
        );

        let only_ax = PermissionsStatus {
            accessibility: true,
            screen_recording: false,
        };
        assert_eq!(
            missing_from_status(only_ax),
            vec![MissingPermission::ScreenRecording]
        );

        let all = PermissionsStatus {
            accessibility: true,
            screen_recording: true,
        };
        assert!(missing_from_status(all).is_empty());
    }

    #[test]
    fn settings_urls_match_swift() {
        // Verbatim parity with PermissionsGate.swift's SettingsPane enum so
        // grant-flows opens the exact same panes on both the Swift and
        // Rust binaries.
        assert_eq!(
            MissingPermission::Accessibility.settings_url(),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        );
        assert_eq!(
            MissingPermission::ScreenRecording.settings_url(),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        );
    }
}
