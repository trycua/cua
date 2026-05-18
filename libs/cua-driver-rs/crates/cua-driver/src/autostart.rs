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
    const REGISTER_PS: &str = r#"
$ErrorActionPreference = 'Stop'
$user = "$env:COMPUTERNAME\$env:USERNAME"
$action = New-ScheduledTaskAction -Execute $env:CUA_DRIVER_AS_EXE -Argument 'serve' -WorkingDirectory $env:USERPROFILE
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit (New-TimeSpan -Hours 0)
Unregister-ScheduledTask -TaskName 'cua-driver-serve' -Confirm:$false -ErrorAction SilentlyContinue
Register-ScheduledTask -TaskName 'cua-driver-serve' -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Description 'cua-driver-rs: serve daemon, auto-start at interactive logon' | Out-Null
"#;

    pub fn enable(exe: &str) -> Result<()> {
        // Pass the binary path via env var so the script doesn't need
        // shell-quoting acrobatics for paths with spaces or odd chars.
        let out = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", REGISTER_PS])
            .env("CUA_DRIVER_AS_EXE", exe)
            .output()
            .map_err(|e| anyhow!("failed to invoke powershell: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!(
                "PowerShell Register-ScheduledTask failed (exit {}): {}",
                out.status.code().unwrap_or(-1),
                stderr.trim()
            ));
        }
        Ok(())
    }

    pub fn disable() -> Result<()> {
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
         libs/cua-driver-rs/scripts/install-local.sh --autostart for the \
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
