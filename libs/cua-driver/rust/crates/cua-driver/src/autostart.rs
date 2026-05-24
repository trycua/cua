//! `cua-driver autostart {enable|disable|status|kick}` — register / inspect /
//! trigger the platform-native auto-start mechanism so `cua-driver serve`
//! comes up on every interactive logon without the user pasting a
//! startup one-liner.
//!
//! ## Platform mapping
//!
//! - **Windows**: Scheduled Task `cua-driver-serve` registered with
//!   `LogonType: Interactive` so it lands in a Session 1+ logon (never
//!   Session 0). Equivalent to what `scripts/install.ps1 -AutoStart`
//!   does — the install script can call out to this subcommand to
//!   keep the registration logic in one place.
//! - **macOS / Linux**: not implemented yet. Returns an error pointing
//!   the user at the manual recipe (`launchctl` / `systemctl --user`).
//!   `scripts/install-local.sh --autostart` covers the manual path
//!   today.
//!
//! ## Why shell out (Windows)
//!
//! The Task Scheduler 2.0 COM surface (`ITaskService`, `ITaskDefinition`,
//! `ITaskFolder`, `IPrincipal`, ...) is ~10 nested COM-wrapper calls in
//! Rust before you've even configured the principal, with multiple BSTR
//! marshalling steps and a lot of "this method takes a VARIANT, that
//! one takes a BSTR" footguns. Shelling out to PowerShell's
//! `Register-ScheduledTask` cmdlet — which itself uses Task Scheduler
//! 2.0 under the hood — gets us identical behavior in 5 lines and stays
//! exactly in lock-step with what `scripts/install.ps1` does (literally
//! the same command). When `install.ps1` evolves, this code follows it
//! for free.

use anyhow::{anyhow, Result};

/// Canonical task / unit name. Used by every platform.
pub const TASK_NAME: &str = "cua-driver-serve";

/// Reported by `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// No autostart entry registered.
    NotRegistered,
    /// Entry registered but not currently running.
    RegisteredIdle,
    /// Entry registered AND a `cua-driver serve` process is live.
    RegisteredRunning,
}

impl Status {
    pub fn tag(self) -> &'static str {
        match self {
            Status::NotRegistered => "not-registered",
            Status::RegisteredIdle => "registered (not running)",
            Status::RegisteredRunning => "registered (running)",
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────

/// Register the platform-native autostart entry for `cua-driver serve`.
/// Idempotent: any existing entry with the same name is replaced.
pub fn enable() -> Result<()> {
    let exe = current_exe_for_autostart()?;
    platform::enable(&exe)
}

/// Remove the autostart entry. No-op if none is registered.
pub fn disable() -> Result<()> {
    platform::disable()
}

/// Report whether the entry is registered and whether the daemon is running.
pub fn status() -> Result<Status> {
    platform::status()
}

/// Run the autostart entry immediately without waiting for a fresh logon.
/// Errors if the entry isn't registered.
pub fn kick() -> Result<()> {
    platform::kick()
}

/// Find the cua-driver executable to bake into the autostart entry.
/// Uses `std::env::current_exe`, canonicalised to its real path (resolves
/// junction / symlink chains so a versioned upgrade flipping `current`
/// stays transparent to the registered task). The resolved path is what
/// gets stored in the Scheduled Task / LaunchAgent / unit file.
fn current_exe_for_autostart() -> Result<String> {
    let exe = std::env::current_exe()
        .map_err(|e| anyhow!("could not resolve current executable: {e}"))?;
    let canonical = std::fs::canonicalize(&exe).unwrap_or(exe);
    let path = canonical.to_string_lossy().into_owned();
    // On Windows, `canonicalize` returns a `\\?\C:\...` extended-length
    // path. PowerShell + the Task Scheduler XML schema both handle it
    // correctly, but it looks alarming in `schtasks /Query` output.
    // Strip the prefix for readability — the unprefixed form is still
    // valid as long as the path fits MAX_PATH (260 chars), which any
    // realistic install will.
    #[cfg(target_os = "windows")]
    let path = path
        .strip_prefix(r"\\?\")
        .map(str::to_owned)
        .unwrap_or(path);
    Ok(path)
}

// ── Windows impl ──────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::process::Command;

    /// Inline PowerShell that mirrors `install.ps1::Register-CuaDriverAutostart`
    /// exactly. Kept as a single one-liner so a quick `gh-blame` diff against
    /// install.ps1 surfaces any divergence; the moment install.ps1 changes
    /// shape, this script needs the same edit.
    ///
    /// **Hidden-console launch (issue #1645):** the task action wraps
    /// `cua-driver.exe serve` in `powershell -WindowStyle Hidden` +
    /// `Start-Process -WindowStyle Hidden`. Without this, Task Scheduler
    /// allocates a new CUI console window (because `cua-driver.exe` is a
    /// console-subsystem binary) that stays visible on the user's desktop
    /// for the lifetime of the daemon. The PowerShell wrapper exits
    /// immediately after spawning the child, leaving only `cua-driver.exe`
    /// in the process tree.
    ///
    /// **RunLevel = Highest** (since 2026-05-21): the daemon is registered to
    /// run at the user's elevated/admin token rather than the filtered
    /// standard-user token. This is what lets the daemon drive UWP /
    /// AppContainer apps (Calculator, modern Settings, Photos, …) — at
    /// Medium IL the cross-AppContainer UIA RPC returns a stub (~1 element
    /// instead of the full tree, see #1602 / #1601). High IL crosses that
    /// boundary cleanly. Trade-off: `Register-ScheduledTask -RunLevel
    /// Highest` requires the caller to already be at High IL, so this
    /// function emits an actionable error when invoked from a non-elevated
    /// shell. Users opt into autostart via the installer's `-AutoStart`
    /// flag or `cua-driver autostart enable`, both of which prompt for
    /// elevation when needed.
    ///
    /// **Account-name format**: on domain-joined machines USERDOMAIN holds the
    /// AD domain name (e.g. CORP) and the principal must be `CORP\username`.
    /// On workgroup machines USERDOMAIN holds either the literal string
    /// "WORKGROUP" or the COMPUTERNAME, and the principal must be
    /// `COMPUTERNAME\username` — `WORKGROUP\username` errors with
    /// "No mapping between account names and security IDs was done". The
    /// $domain selector below picks USERDOMAIN when it's a real
    /// (non-WORKGROUP, non-COMPUTERNAME) domain and falls back to
    /// COMPUTERNAME otherwise, covering both shapes.
    const REGISTER_PS: &str = r#"
$ErrorActionPreference = 'Stop'
if ($env:USERDOMAIN -and $env:USERDOMAIN -ne 'WORKGROUP' -and $env:USERDOMAIN -ne $env:COMPUTERNAME) {
    $domain = $env:USERDOMAIN
} else {
    $domain = $env:COMPUTERNAME
}
$user = "$domain\$env:USERNAME"
# Use a hidden PowerShell wrapper as the task action so Windows never
# allocates a visible console window when the daemon is launched at
# logon. cua-driver.exe is a CUI (console-subsystem) binary: without
# this wrapper, Task Scheduler allocates a new console and the window
# stays on the user's desktop for the lifetime of the daemon (issue #1645).
# Start-Process -WindowStyle Hidden spawns the child fully detached;
# the powershell.exe wrapper exits immediately after, leaving only the
# cua-driver.exe daemon in the process tree.
$action = New-ScheduledTaskAction `
    -Execute 'powershell.exe' `
    -Argument "-NoProfile -WindowStyle Hidden -NonInteractive -Command `"Start-Process -FilePath '$env:CUA_DRIVER_AS_EXE' -ArgumentList 'serve' -WindowStyle Hidden -WorkingDirectory '$env:USERPROFILE'`"" `
    -WorkingDirectory $env:USERPROFILE
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit (New-TimeSpan -Hours 0)
Unregister-ScheduledTask -TaskName 'cua-driver-serve' -Confirm:$false -ErrorAction SilentlyContinue
Register-ScheduledTask -TaskName 'cua-driver-serve' -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Description 'cua-driver-rs: serve daemon, auto-start at interactive logon, RunLevel=Highest for UWP/AppContainer support' | Out-Null

# Note: the uiAccess'd worker (`cua-driver-uia.exe`) does NOT get its own
# scheduled task. uiAccess PEs can only be launched via ShellExecute, and
# Task Scheduler's PowerShell-wrapper Action path returns ERROR_NOT_LOGGED_ON
# (0x800710E0). Instead, `cua-driver serve` itself spawns the sibling worker
# via ShellExecuteEx at startup (see serve.rs `maybe_spawn_uia_worker`),
# which works because the spawn originates from a Session-2 process with an
# interactive desktop. See #1602.
"#;

    pub fn enable(exe: &str) -> Result<()> {
        // First attempt: register directly. Works when called from an
        // already-elevated context (install.ps1's self-elevated child shell,
        // or a user running cua-driver autostart enable from an admin
        // PowerShell). Pass the binary path via env var so the script
        // doesn't need shell-quoting acrobatics for paths with spaces.
        let out = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", REGISTER_PS])
            .env("CUA_DRIVER_AS_EXE", exe)
            .output()
            .map_err(|e| anyhow!("failed to invoke powershell: {e}"))?;
        if out.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&out.stderr);
        let stderr_lower = stderr.to_lowercase();
        let looks_like_access_denied = stderr_lower.contains("access is denied")
            || stderr_lower.contains("0x80070005")
            || stderr_lower.contains("permission")
            || stderr_lower.contains("requires elevation");

        if !looks_like_access_denied {
            return Err(anyhow!(
                "PowerShell Register-ScheduledTask failed (exit {}): {}",
                out.status.code().unwrap_or(-1),
                stderr.trim()
            ));
        }

        // Access-denied — caller is at Medium IL but the task wants Highest.
        // Self-elevate via ShellExecute "runas" verb, which fires the UAC
        // prompt. The elevated child runs the same registration via
        // powershell -Command, with the registered binary path injected as
        // an env var (durable across the elevation boundary).
        eprintln!("cua-driver: registering autostart at RunLevel=Highest needs admin.");
        eprintln!("cua-driver: triggering UAC prompt — accept it to register the task.");

        let elevated_status = {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW = 0x08000000 — keep the elevated child's
            // console out of the foreground; PowerShell already runs hidden
            // via Start-Process -Verb RunAs's WindowStyle Hidden.
            const CREATE_NO_WINDOW: u32 = 0x08000000;

            // PowerShell incantation: Start-Process -Verb RunAs to elevate,
            // run our own exe with `autostart enable` so the registration
            // happens INSIDE the elevated process (where it'll succeed on
            // the first attempt and not re-enter this branch). -Wait so we
            // can capture the child's exit code.
            let inner = format!(
                "& \"{}\" autostart enable",
                exe.replace('"', "`\"")
            );
            let outer = format!(
                "$p = Start-Process -FilePath '{}' -ArgumentList @('-NoProfile','-NonInteractive','-Command',{}) -Verb RunAs -Wait -PassThru; exit $p.ExitCode",
                "powershell.exe",
                // Embed the inner command as a single-quoted PowerShell string
                // (PS escapes ' as '' inside ''-strings).
                format!("'{}'", inner.replace('\'', "''"))
            );
            Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command", &outer])
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .map_err(|e| anyhow!("failed to spawn elevation helper: {e}"))?
        };

        if elevated_status.success() {
            Ok(())
        } else {
            Err(anyhow!(
                "self-elevation for autostart registration failed (exit {}). The UAC \
                 prompt was probably dismissed. Re-run `cua-driver autostart enable` \
                 and accept the prompt, or run install.ps1 -AutoStart which has the \
                 same self-elevation flow. See https://github.com/trycua/cua/issues/1602.",
                elevated_status.code().unwrap_or(-1)
            ))
        }
    }

    pub fn disable() -> Result<()> {
        // Tear down any legacy `cua-driver-uia` task from earlier autostart
        // versions that registered a separate scheduled task for the uia
        // worker. Current versions don't register one (serve.rs spawns the
        // worker as a child), but `disable` should still clean up old
        // installs. Best-effort — "task not found" is ignored.
        let _ = Command::new("schtasks")
            .args(["/Delete", "/TN", "cua-driver-uia", "/F"])
            .output();

        // schtasks /Delete returns 0 on success, 1 on "task not found"
        // (which we treat as success: the goal is "no task registered"
        // and it already isn't). Match on stderr text rather than exit
        // code because schtasks doesn't distinguish "doesn't exist" from
        // "permission denied" via exit code.
        let out = Command::new("schtasks")
            .args(["/Delete", "/TN", TASK_NAME, "/F"])
            .output()
            .map_err(|e| anyhow!("failed to invoke schtasks: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let combined = format!("{stdout}{stderr}").to_lowercase();
        if combined.contains("does not exist")
            || combined.contains("cannot find the file specified")
            || combined.contains("the system cannot find")
        {
            return Ok(());
        }
        Err(anyhow!(
            "schtasks /Delete failed (exit {}): {}",
            out.status.code().unwrap_or(-1),
            stderr.trim()
        ))
    }

    pub fn status() -> Result<Status> {
        // schtasks /Query exits 0 with task details on stdout, or 1 with
        // "ERROR: The system cannot find the file specified." on stderr.
        let out = Command::new("schtasks")
            .args(["/Query", "/TN", TASK_NAME])
            .output()
            .map_err(|e| anyhow!("failed to invoke schtasks: {e}"))?;
        if !out.status.success() {
            return Ok(Status::NotRegistered);
        }
        // Registered — now check whether `cua-driver serve` is running.
        // Avoid invoking `tasklist` (slow ~200ms on first run); use the
        // same registry the daemon's own status command uses via a
        // direct check on the named pipe.
        if crate::serve::is_daemon_listening(&crate::serve::default_socket_path()) {
            Ok(Status::RegisteredRunning)
        } else {
            Ok(Status::RegisteredIdle)
        }
    }

    pub fn kick() -> Result<()> {
        let out = Command::new("schtasks")
            .args(["/Run", "/TN", TASK_NAME])
            .output()
            .map_err(|e| anyhow!("failed to invoke schtasks: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!(
                "schtasks /Run failed (exit {}): {}",
                out.status.code().unwrap_or(-1),
                stderr.trim()
            ));
        }
        Ok(())
    }
}

// ── macOS / Linux stubs ───────────────────────────────────────────────────

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::*;

    const NOT_YET: &str =
        "cua-driver autostart is currently Windows-only. macOS users: see \
         libs/cua-driver/rust/scripts/install-local.sh --autostart for the \
         LaunchAgent recipe. Linux users: same script registers a systemd \
         --user unit. A cross-platform impl is tracked as a follow-up.";

    pub fn enable(_exe: &str) -> Result<()> {
        Err(anyhow!(NOT_YET))
    }
    pub fn disable() -> Result<()> {
        Err(anyhow!(NOT_YET))
    }
    pub fn status() -> Result<Status> {
        Err(anyhow!(NOT_YET))
    }
    pub fn kick() -> Result<()> {
        Err(anyhow!(NOT_YET))
    }
}

// ── CLI dispatcher ────────────────────────────────────────────────────────

/// `cua-driver autostart <subcommand>` entry point. Prints user-facing
/// output and exits the process via `std::process::exit` so the caller
/// (main) doesn't need to plumb back an exit code for every subcommand.
pub fn run_autostart_cmd(subcommand: &str) {
    let (verb_result, success_text): (Result<()>, String) = match subcommand {
        "enable" => (enable(), format!(
            "Registered autostart entry '{TASK_NAME}'.\n  \
             cua-driver serve will start at every interactive logon."
        )),
        "disable" => (disable(), format!(
            "Removed autostart entry '{TASK_NAME}' (no-op if it was already absent)."
        )),
        "status" => match status() {
            Ok(s) => {
                println!("{}", s.tag());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("cua-driver autostart status: {e}");
                std::process::exit(1);
            }
        },
        "kick" => (kick(), format!(
            "Started autostart entry '{TASK_NAME}' for the current session."
        )),
        other => {
            eprintln!("Unknown autostart subcommand: {other:?}");
            eprintln!("Usage: cua-driver autostart {{enable|disable|status|kick}}");
            std::process::exit(64);
        }
    };
    match verb_result {
        Ok(()) => {
            println!("{success_text}");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("cua-driver autostart {subcommand}: {e}");
            std::process::exit(1);
        }
    }
}
