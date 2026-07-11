//! First-launch permissions gate.
//!
//! Bundled app launches use the native panel in [`super::panel`]. It explains
//! both grants before any system prompt, requests one grant per user action,
//! and refreshes live until setup completes. Bare binaries and headless
//! launches use this module's terminal fallback:
//!
//!   1. Inspect TCC state on `serve` startup.
//!   2. If any required grant is missing, print a clear explanation
//!      (which grant, why cua-driver needs it, what to do next).
//!   3. Request the missing grants and open the relevant System Settings panes via the
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
//! Embedded mode bypasses both surfaces because the host application owns
//! permission attribution and onboarding.

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
            Self::Accessibility => {
                "lets the driver read interface text and operate buttons, fields, menus, \
                 and other app controls."
            }
            Self::ScreenRecording => {
                "lets the driver see app windows so actions can be placed accurately \
                 and their results verified."
            }
        }
    }

    /// `x-apple.systempreferences:` URL that deep-links into the matching
    /// Privacy pane.  Same strings the Swift gate uses.
    pub fn settings_url(self) -> &'static str {
        match self {
            Self::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            Self::ScreenRecording => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
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
    /// `off`, case-insensitive, disables the gate), an explicit
    /// `--no-permissions-gate` flag, and embedded mode
    /// (`CUA_DRIVER_EMBEDDED=1`).  Any signal is sufficient to opt out.
    /// Embedded mode opts out because the host app owns the grant flow;
    /// the driver must never raise its own prompts.
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
            opt_out: no_gate_flag || env_disabled || cua_driver_core::embedded_mode(),
            ..Self::default()
        }
    }
}

/// Set by `reexec_self` on the restarted process so its `run_if_needed`
/// polls SILENTLY instead of re-raising the TCC prompts on every ~25s
/// re-exec (which otherwise spams the user with "Cua Driver" dialogs).
const GATE_REEXEC_ENV: &str = "CUA_DRIVER_RS_GATE_REEXEC";

/// Persists the gate's original start time (unix seconds) across re-execs
/// so the `deadline` is cumulative. Without it each re-exec'd process resets
/// `start`, and since a re-exec fires (~25s) well before the deadline
/// (~10min) the deadline never triggers and the daemon re-execs forever.
const GATE_START_ENV: &str = "CUA_DRIVER_RS_GATE_START_UNIX";

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReexecMode {
    /// Return to the native panel after a fresh TCC preflight. This mode must
    /// not set the silent-wait marker: a denied decision needs recoverable
    /// per-row actions, not an invisible polling loop.
    InteractivePanel,
    /// Terminal fallback cache refresh. The restarted process resumes the
    /// existing wait without raising prompts or reopening presentation UI.
    SilentWait,
}

/// Run the gate if needed.  When called and the process already has both
/// grants, this returns immediately without printing anything — the
/// `serve` happy path is unaffected.
///
/// When grants are missing and `opt_out` is false, bundled launches present
/// the native per-row flow. If the native panel is unavailable, the terminal
/// fallback prints its explanation, optionally raises TCC prompts, opens the
/// matching Settings panes, and polls until everything is green or the
/// deadline elapses.
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
        // Clear one-shot re-exec bookkeeping so later child processes do not
        // inherit a stale "silent refresh" state.
        std::env::remove_var(GATE_REEXEC_ENV);
        std::env::remove_var(GATE_START_ENV);
        #[cfg(target_os = "macos")]
        crate::permissions::panel::clear_request_history();
        return Ok(());
    }

    // A gate re-exec (`wait_for_grants` restarts the daemon ~every 25s to
    // refresh the per-process TCC trust cache) re-runs this function. The
    // FIRST process already raised the TCC prompts + showed the panel;
    // re-doing that on every re-exec spams a fresh "Cua Driver" dialog at the
    // user (forever, since a stale/never-granted state keeps re-execing). A
    // re-exec'd process polls SILENTLY instead — skip the prompts + panel and
    // go straight to the wait loop, which re-checks the grant and re-execs.
    if std::env::var(GATE_REEXEC_ENV).is_ok() {
        return wait_for_grants(&opts);
    }

    let missing = missing_from_status(initial);

    // Bundled launches show the explanation before requesting either TCC
    // grant. The panel owns its per-row prompts and Settings deep links, so
    // system UI never appears without an explicit Allow click.
    match present_panel_if_available(initial) {
        PanelPresentation::ShownAllGranted => {
            // Start one fresh process image after first-time setup. macOS can
            // cache both AX trust and Screen Recording capability inside the
            // requesting process, and some OS versions do not make capture
            // usable until relaunch. `execvp` preserves argv and attribution;
            // the new image sees both grants on its fast path and does not
            // show the panel again. It also avoids asking the user to quit and
            // manually reconstruct the daemon launch arguments.
            #[cfg(target_os = "macos")]
            reexec_self(ReexecMode::InteractivePanel);
            return wait_for_grants(&opts);
        }
        PanelPresentation::ShownRefreshRequired => {
            // Returning from a TCC decision or System Settings is a progress
            // signal, not proof of a grant. Re-exec immediately so cached
            // false probes cannot strand the panel. A denied grant reopens
            // the panel in its recoverable “Complete in System Settings”
            // state; a granted one advances from fresh status.
            #[cfg(target_os = "macos")]
            reexec_self(ReexecMode::InteractivePanel);
            return wait_for_grants(&opts);
        }
        PanelPresentation::ShownDismissed => {
            return Ok(());
        }
        PanelPresentation::NotShown => {}
    }

    // Bare-binary and headless launches retain the terminal fallback. Print
    // its explanation before raising prompts so the user knows why macOS is
    // asking. Embedded mode returned at the top of this function and can
    // never reach these calls.
    print_banner(&missing, opts.open_settings);
    if opts.also_raise_prompts {
        if missing.contains(&MissingPermission::Accessibility) {
            let _ = request_accessibility();
        }
        if missing.contains(&MissingPermission::ScreenRecording) {
            let _ = request_screen_recording();
        }
    }
    if opts.open_settings {
        for permission in &missing {
            if let Err(error) = open_system_settings_for(*permission) {
                eprintln!(
                    "  (could not auto-open Settings for {}: {error})",
                    permission.label()
                );
            }
        }
    }
    wait_for_grants(&opts)
}

/// Outcome of a panel-present attempt. Drives the subsequent flow in
/// [`run_if_needed`] — see comments at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelPresentation {
    /// Panel could not be shown (opt-out env var, bare-binary launch,
    /// headless, etc.). Caller should fall back to the terminal banner.
    NotShown,
    /// Panel shown; user closed the window.
    ShownDismissed,
    /// Panel shown; its 1 Hz poll loop saw both grants flip green and
    /// dismissed automatically. Caller can skip the trailing wait loop.
    ShownAllGranted,
    /// Panel shown; the user returned from system permission UI. The result
    /// is deliberately unknown until a fresh process verifies it.
    ShownRefreshRequired,
}

fn present_panel_if_available(initial: PermissionsStatus) -> PanelPresentation {
    #[cfg(target_os = "macos")]
    {
        use crate::permissions::panel;
        if !panel::panel_enabled() {
            return PanelPresentation::NotShown;
        }
        match panel::show_modal(panel::PanelOpts {
            initial_status: initial,
        }) {
            panel::PanelOutcome::Unavailable => PanelPresentation::NotShown,
            panel::PanelOutcome::Dismissed => PanelPresentation::ShownDismissed,
            panel::PanelOutcome::AllGranted => PanelPresentation::ShownAllGranted,
            panel::PanelOutcome::RefreshRequired => PanelPresentation::ShownRefreshRequired,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = initial;
        PanelPresentation::NotShown
    }
}

/// Block until all required permissions are granted or the deadline
/// elapses.  Emits a status line every `opts.status_interval` while
/// waiting so the user has feedback.
///
/// Exposed separately from [`run_if_needed`] for callers that want to
/// drive the wait phase manually (e.g. tests that pre-skip the banner).
///
/// macOS quirk: `AXIsProcessTrusted()` and `CGPreflightScreenCaptureAccess()`
/// cache their results per-process. The cache survives both
/// `tccutil reset` and live `System Settings` toggle flips — once a
/// process has seen `false`, polling will keep returning `false`
/// forever for that process even after the user grants the permission.
///
/// Pumping the AppKit run loop (which we tried first) does not flush
/// the cache. The only reliable fix is to **re-execute the binary**
/// (`execvp` the current image with the same argv): the new process
/// image gets a fresh TCC client and re-queries tccd cleanly.
///
/// Strategy: after every `EXEC_AFTER_POLLS` polls without progress,
/// we exec ourselves. Light enough (process start is fast) that this
/// is invisible to the user — they just see a "rechecking" status
/// line. If the grant has been given the new process picks it up
/// instantly; otherwise it falls back into the same wait loop.
pub fn wait_for_grants(opts: &GateOpts) -> Result<()> {
    // Anchor the deadline to the ORIGINAL gate start, persisted across
    // re-execs via GATE_START_ENV. A re-exec'd process shifts `start` back by
    // the elapsed wall time so `start.elapsed()` keeps growing across
    // restarts and the deadline is cumulative — otherwise each restart resets
    // the clock, re-exec fires before the deadline, and the gate (and the
    // daemon + cursor overlay it restarts) churns forever when ungranted.
    let start = {
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        match std::env::var(GATE_START_ENV)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
        {
            Some(orig) => Instant::now()
                .checked_sub(Duration::from_secs(now_unix.saturating_sub(orig)))
                .unwrap_or_else(Instant::now),
            None => {
                std::env::set_var(GATE_START_ENV, now_unix.to_string());
                Instant::now()
            }
        }
    };
    let mut last_status_print = start;
    let mut last_missing: Vec<MissingPermission> = check_required_permissions();
    let mut polls_without_change: u32 = 0;

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

        // On macOS, when polling has gone several iterations without
        // any change, re-exec the binary to invalidate the per-process
        // TCC cache. See the function-level doc for the rationale.
        //
        // Since #1761 the serve loop runs concurrently with this gate on
        // a background thread, so every `reexec_self()` restarts the whole
        // daemon — including re-binding the Unix socket. A tight re-exec
        // cadence would therefore make the socket flap (clients see brief
        // connection failures on each restart). We trade grant-detection
        // latency (fine for the grant flow — the user is clicking through
        // System Settings on a human timescale) for socket stability:
        // ~25 polls (~25s) between re-execs instead of ~5s.
        #[cfg(target_os = "macos")]
        {
            const EXEC_AFTER_POLLS: u32 = 25;
            if polls_without_change >= EXEC_AFTER_POLLS {
                println!(
                    "[cua-driver] rechecking permissions — restarting daemon \
                     (still missing: {})",
                    fmt_missing(&last_missing)
                );
                let _ = std::io::stdout().flush();
                reexec_self(ReexecMode::SilentWait);
                // `reexec_self` only returns on failure; if it does,
                // continue the loop with a reset counter so we don't
                // spin-exec.
                polls_without_change = 0;
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
            polls_without_change = 0;
            continue;
        }
        polls_without_change = polls_without_change.saturating_add(1);

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

/// Replace the current process image with a fresh copy of the same
/// binary + argv via `execvp(2)`. Used by [`wait_for_grants`] on
/// macOS to invalidate the per-process TCC trust cache after the
/// user has granted permissions in System Settings — `AXIsProcessTrusted`
/// won't see the grant inside the original process otherwise.
///
/// `execvp` only returns on failure; on success, control transfers to
/// the new process image at the C `main` entry and the rest of this
/// function is never executed. Caller continues if the call fails so
/// the gate keeps polling rather than aborting hard.
#[cfg(target_os = "macos")]
fn reexec_self(mode: ReexecMode) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStringExt;

    // Resolve the path to ourselves. `current_exe` returns the actual
    // binary path (e.g. /Applications/CuaDriver.app/Contents/MacOS/
    // cua-driver) which is what we want — execvp searches PATH for
    // bare names, and we don't want a stale `~/.local/bin/cua-driver`
    // symlink to win.
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("[cua-driver] re-exec failed: cannot resolve current_exe");
        return;
    };
    let argv: Vec<CString> = std::env::args_os()
        .filter_map(|a| CString::new(a.into_vec()).ok())
        .collect();
    if argv.is_empty() {
        eprintln!("[cua-driver] re-exec failed: empty argv");
        return;
    }

    // execvp takes a NULL-terminated argv. Build pointers + sentinel.
    let mut argv_ptrs: Vec<*const libc::c_char> = argv.iter().map(|s| s.as_ptr()).collect();
    argv_ptrs.push(std::ptr::null());

    let exe_c = match CString::new(exe.into_os_string().into_vec()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[cua-driver] re-exec failed: exe path contains NUL ({e})");
            return;
        }
    };

    // Mark the restarted process as a gate re-exec so its `run_if_needed`
    // polls silently (no re-prompt), and anchor the original gate start so
    // the deadline is cumulative across re-execs. execvp inherits the
    // environment, so the new image sees these.
    prepare_reexec_environment(mode);

    // execvp returns -1 on failure; on success it does not return.
    // SAFETY: argv_ptrs is NULL-terminated; exe_c outlives the call.
    unsafe {
        libc::execvp(exe_c.as_ptr(), argv_ptrs.as_ptr());
    }
    // If we get here the exec failed.
    let err = std::io::Error::last_os_error();
    eprintln!("[cua-driver] re-exec failed: {err}");
}

#[cfg(target_os = "macos")]
fn prepare_reexec_environment(mode: ReexecMode) {
    match mode {
        ReexecMode::InteractivePanel => std::env::remove_var(GATE_REEXEC_ENV),
        ReexecMode::SilentWait => std::env::set_var(GATE_REEXEC_ENV, "1"),
    }
    if std::env::var(GATE_START_ENV).is_err() {
        if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            std::env::set_var(GATE_START_ENV, d.as_secs().to_string());
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

    /// Crate-wide env-var test lock — `from_env_and_flag` reads
    /// `CUA_DRIVER_EMBEDDED`, which the `check_permissions` tests mutate.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::permissions::test_env_lock()
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
        std::env::remove_var(cua_driver_core::EMBEDDED_ENV);
        let opts = GateOpts::from_env_and_flag(false);
        assert!(!opts.opt_out);
    }

    #[test]
    fn embedded_mode_opts_out_of_gate() {
        let _guard = env_lock();
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_GATE");
        std::env::set_var(cua_driver_core::EMBEDDED_ENV, "1");
        assert!(GateOpts::from_env_and_flag(false).opt_out);
        // Only the exact value "1" enables embedded mode.
        std::env::set_var(cua_driver_core::EMBEDDED_ENV, "true");
        assert!(!GateOpts::from_env_and_flag(false).opt_out);
        std::env::remove_var(cua_driver_core::EMBEDDED_ENV);
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
            let expected_opt_out = matches!(
                v.to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            );
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

    #[cfg(target_os = "macos")]
    #[test]
    fn interactive_refresh_reopens_panel_after_reexec() {
        let _guard = env_lock();
        std::env::set_var(GATE_REEXEC_ENV, "1");
        std::env::remove_var(GATE_START_ENV);
        prepare_reexec_environment(ReexecMode::InteractivePanel);
        assert!(
            std::env::var(GATE_REEXEC_ENV).is_err(),
            "permission-decision refresh must not fall into silent polling"
        );
        assert!(std::env::var(GATE_START_ENV).is_ok());
        std::env::remove_var(GATE_START_ENV);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn terminal_cache_refresh_remains_silent_after_reexec() {
        let _guard = env_lock();
        std::env::remove_var(GATE_REEXEC_ENV);
        std::env::remove_var(GATE_START_ENV);
        prepare_reexec_environment(ReexecMode::SilentWait);
        assert_eq!(std::env::var(GATE_REEXEC_ENV).as_deref(), Ok("1"));
        std::env::remove_var(GATE_REEXEC_ENV);
        std::env::remove_var(GATE_START_ENV);
    }
}
