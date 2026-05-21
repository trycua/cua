# cua-driver-rs uninstaller (Windows) — removes everything install.ps1
# laid down: the Scheduled Task autostart entry, running daemon
# processes, the directory junctions wiring the visible bin dir back to
# a per-version release dir, the entire package home tree, and any skill
# junctions the binary's `skills install` verb dropped under the agent
# config directories.
#
# Usage (one-liner — recommended):
#   irm https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/uninstall.ps1 | iex
#
# Force (no prompts):
#   $forceArgs = @('-Force')
#   & ([scriptblock]::Create((irm https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/uninstall.ps1))) @forceArgs
#
# What gets removed:
#   - Scheduled Task 'cua-driver-serve' (autostart entry registered by
#     `cua-driver autostart enable` or install.ps1 -AutoStart)
#   - Any running cua-driver.exe processes (so file handles don't pin
#     the binary directory open during the delete pass)
#   - <visibleBinDir>     = %LOCALAPPDATA%\Programs\trycua\cua-driver-rs\bin  (directory junction)
#   - <currentDir>        = %USERPROFILE%\.cua-driver-rs\packages\current     (directory junction)
#   - <packageHome>       = %USERPROFILE%\.cua-driver-rs\                     (entire tree:
#                                                                              releases, lockfile,
#                                                                              telemetry id,
#                                                                              install marker,
#                                                                              version_check.json)
#   - Skill junctions under:
#       %USERPROFILE%\.claude\skills\cua-driver-rs
#       %USERPROFILE%\.agents\skills\cua-driver-rs
#       %USERPROFILE%\.openclaw\skills\cua-driver-rs
#       %APPDATA%\opencode\skills\cua-driver-rs
#     (each only when it's a reparse point — never clobber a real dir).
#
# Conservative on Claude MCP cleanup: we DON'T auto-edit %USERPROFILE%\
# .claude.json on Windows (mirrors the macOS uninstall.sh's stance for
# environments without python3). The closing message prints the
# `claude mcp remove cua-driver-rs` command for the user to run.
#
# Env overrides (mirror install.ps1's variable names):
#   $env:CUA_DRIVER_RS_INSTALL_DIR   visible bin dir to remove
#                                    (default %LOCALAPPDATA%\Programs\trycua\cua-driver-rs\bin)
#   $env:CUA_DRIVER_RS_HOME          package home to remove
#                                    (default %USERPROFILE%\.cua-driver-rs)
#
# Params:
#   -Force      non-interactive: skip the "remove? [y/N]" prompt before
#               each major delete. The one-liner is interactive by
#               default so a stray paste doesn't accidentally wipe a
#               working install.

[CmdletBinding()]
param(
    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# ---------- Path resolution (mirrors install.ps1) -------------------------

if ($env:CUA_DRIVER_RS_INSTALL_DIR) {
    $VisibleBinDir = $env:CUA_DRIVER_RS_INSTALL_DIR
} else {
    $VisibleBinDir = Join-Path $env:LOCALAPPDATA "Programs\trycua\cua-driver-rs\bin"
}

if ($env:CUA_DRIVER_RS_HOME) {
    $HomeDir = $env:CUA_DRIVER_RS_HOME
} else {
    $HomeDir = Join-Path $env:USERPROFILE ".cua-driver-rs"
}

$PackagesDir  = Join-Path $HomeDir   "packages"
$CurrentDir   = Join-Path $PackagesDir "current"
$AutoStartTask = "cua-driver-serve"

# Skill junctions — mirrors the AGENTS list in
# libs/cua-driver-rs/crates/cua-driver/src/skills.rs (the verb that
# creates them) so we remove from the same paths.
$SkillJunctions = @(
    (Join-Path $env:USERPROFILE ".claude\skills\cua-driver-rs"),
    (Join-Path $env:USERPROFILE ".agents\skills\cua-driver-rs"),
    (Join-Path $env:USERPROFILE ".openclaw\skills\cua-driver-rs"),
    (Join-Path $env:APPDATA      "opencode\skills\cua-driver-rs")
)

# ---------- Log helpers ----------------------------------------------------

function Write-Step($message) {
    Write-Host "==> $message"
}

function Write-WarningStep($message) {
    Write-Host "WARNING: $message" -ForegroundColor Yellow
}

function Write-ErrorStep($message) {
    Write-Host "error: $message" -ForegroundColor Red
}

# ---------- Reparse-point helpers -----------------------------------------
#
# install.ps1 wires the bin\ and current\ directories with NTFS directory
# junctions (IO_REPARSE_TAG_MOUNT_POINT). Test-IsReparsePoint differentiates
# them from real directories so we only ever Remove-Item a path the
# installer could have created — never clobber a user's hand-managed dir.

function Test-IsReparsePoint([string]$path) {
    if (-not (Test-Path -LiteralPath $path)) { return $false }
    try {
        $item = Get-Item -LiteralPath $path -Force -ErrorAction Stop
    } catch {
        return $false
    }
    return (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)
}

# ---------- Confirmation prompt -------------------------------------------
#
# The one-liner runs interactive by default so a stray paste doesn't wipe
# a working install — Confirm-Remove gates each major delete. -Force
# (passed at param parse time) skips every prompt, which is what CI / a
# scripted teardown wants.

function Confirm-Remove([string]$what) {
    if ($Force) { return $true }
    # PowerShell's Host.UI prompt handles non-interactive shells (e.g.
    # piped from `irm | iex` in some hosts) by reading from stdin —
    # which is the same channel the script body was just piped through.
    # Fall back to treating an empty / non-y response as "no" so the
    # default is safe.
    $resp = Read-Host "Remove $what ? [y/N]"
    return ($resp -match '^(y|yes)$')
}

# ---------- Main -----------------------------------------------------------

Write-Step "cua-driver-rs uninstaller (Windows)"
Write-Step "  bin dir     : $VisibleBinDir"
Write-Step "  package home: $HomeDir"
Write-Host ""

# 1. Scheduled Task autostart (registered by `cua-driver autostart enable`
#    or install.ps1 -AutoStart). Idempotent — schtasks /Query returns
#    non-zero AND writes stderr when the task is absent. Under PS 5.1 with
#    $ErrorActionPreference=Stop (set at the top of this script), native
#    command stderr becomes a terminating error even when we redirect with
#    `2>$null` — the redirect suppresses display but the error record is
#    still emitted into the error stream. Locally lower ErrorActionPreference
#    around the native call so the missing-task case is non-fatal.
$prevEAP = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
try {
    $taskQuery = & schtasks.exe /Query /TN $AutoStartTask 2>$null
    $taskExitCode = $LASTEXITCODE
} finally {
    $ErrorActionPreference = $prevEAP
}
if ($taskExitCode -eq 0 -and $taskQuery) {
    if (Confirm-Remove "scheduled task '$AutoStartTask' (autostart at logon)") {
        $ErrorActionPreference = 'Continue'
        try {
            & schtasks.exe /Delete /TN $AutoStartTask /F 2>$null | Out-Null
            $delExit = $LASTEXITCODE
        } finally {
            $ErrorActionPreference = $prevEAP
        }
        if ($delExit -eq 0) {
            Write-Step "removed scheduled task $AutoStartTask"
        } else {
            Write-WarningStep "schtasks /Delete /TN $AutoStartTask returned $delExit"
        }
    } else {
        Write-Step "skipped scheduled task $AutoStartTask (user declined)"
    }
} else {
    Write-Step "no scheduled task '$AutoStartTask' registered (skipping)"
}

# 2. Running cua-driver.exe processes. The serve daemon and any active
#    `cua-driver` invocation hold file handles to the binary, which
#    pin the directory junction's target open and make Remove-Item
#    fail with "in use". Stop them up front so subsequent deletes
#    aren't racy.
$running = @(Get-Process -Name "cua-driver" -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    Write-Step "stopping $($running.Count) running cua-driver.exe process(es)"
    foreach ($p in $running) {
        try {
            Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        } catch {
            Write-WarningStep "could not stop pid $($p.Id): $($_.Exception.Message)"
        }
    }
    # Brief pause so the kernel finishes tearing down the process and
    # releases its file handles before we try to delete the binary dir.
    Start-Sleep -Milliseconds 250
} else {
    Write-Step "no running cua-driver.exe processes"
}

# 3. Visible bin directory junction. Only remove when it's actually a
#    reparse point — refuse to clobber a real directory the user might
#    have at that path.
if (Test-Path -LiteralPath $VisibleBinDir) {
    if (Test-IsReparsePoint $VisibleBinDir) {
        if (Confirm-Remove "directory junction $VisibleBinDir") {
            # Remove-Item on a reparse point removes the reparse point
            # itself, NOT the contents of the target. -Force is needed
            # to delete a non-empty junction; -Recurse is harmless
            # against a junction (we're removing the link, not the
            # tree it points to).
            Remove-Item -LiteralPath $VisibleBinDir -Force -Recurse -ErrorAction SilentlyContinue
            Write-Step "removed junction $VisibleBinDir"
        } else {
            Write-Step "skipped $VisibleBinDir (user declined)"
        }
    } else {
        Write-WarningStep "$VisibleBinDir exists but is not a reparse point — refusing to remove."
        Write-WarningStep "  install.ps1 only creates junctions at this path, so this is likely a hand-managed directory."
    }
} else {
    Write-Step "no junction at $VisibleBinDir (skipping)"
}

# 4. current\ directory junction inside the package home. Same
#    reparse-point check as above — never clobber a real dir.
if (Test-Path -LiteralPath $CurrentDir) {
    if (Test-IsReparsePoint $CurrentDir) {
        Remove-Item -LiteralPath $CurrentDir -Force -Recurse -ErrorAction SilentlyContinue
        Write-Step "removed junction $CurrentDir"
    } else {
        Write-WarningStep "$CurrentDir exists but is not a reparse point — leaving it for the package-home pass below."
    }
}

# 5. Entire package home ($HomeDir). Contains releases\, lockfile,
#    telemetry id, install marker, version_check.json, and the now-
#    removed current\ junction.
if (Test-Path -LiteralPath $HomeDir) {
    if (Confirm-Remove "package home tree $HomeDir (releases, lockfile, telemetry id, install marker)") {
        # -Recurse -Force walks into every subdir and clears read-only
        # bits. ErrorAction SilentlyContinue tolerates leftover handles
        # (rare after step 2's process kill); we log a follow-up if
        # anything survived.
        Remove-Item -LiteralPath $HomeDir -Force -Recurse -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $HomeDir) {
            Write-WarningStep "$HomeDir was not fully removed — some files may still be locked."
            Write-WarningStep "  Close any open cua-driver processes / shells with cwd inside the tree and re-run."
        } else {
            Write-Step "removed $HomeDir"
        }
    } else {
        Write-Step "skipped $HomeDir (user declined)"
    }
} else {
    Write-Step "no package home at $HomeDir (skipping)"
}

# 6. Skill junctions. Only remove reparse points — leave a real dir
#    in place (a user with a hand-managed cua-driver-rs skill dir
#    gets to keep it). Same defensive shape as Linux/macOS.
foreach ($skillLink in $SkillJunctions) {
    if (Test-Path -LiteralPath $skillLink) {
        if (Test-IsReparsePoint $skillLink) {
            Remove-Item -LiteralPath $skillLink -Force -Recurse -ErrorAction SilentlyContinue
            Write-Step "removed skill junction $skillLink"
        } else {
            Write-Step "$skillLink is a real directory (not a reparse point) — skipping"
        }
    } else {
        Write-Step "no skill junction at $skillLink (skipping)"
    }
}

# ---------- Closing message -----------------------------------------------

Write-Host ""
Write-Host "cua-driver-rs uninstalled." -ForegroundColor Green
Write-Host ""
Write-Host "Claude Code MCP registrations:" -ForegroundColor Yellow
Write-Host "  We don't auto-edit ~/.claude.json on Windows. If you registered cua-driver-rs"
Write-Host "  with Claude Code, remove it manually:"
Write-Host ""
Write-Host "    claude mcp remove cua-driver-rs"
Write-Host ""
Write-Host "  Or edit ~/.claude.json directly and delete entries whose 'command' points at"
Write-Host "  cua-driver.exe under %LOCALAPPDATA%\Programs\trycua\cua-driver-rs\bin\."
Write-Host ""
Write-Host "PATH:"
Write-Host "  If you added $VisibleBinDir to your User PATH after the install, remove it:"
Write-Host ""
Write-Host "    `$old = [Environment]::GetEnvironmentVariable('Path', 'User')"
Write-Host "    `$new = ((`$old.Split(';')) | Where-Object { `$_ -and (`$_.TrimEnd('\') -ne '$($VisibleBinDir.TrimEnd('\'))') }) -join ';'"
Write-Host "    [Environment]::SetEnvironmentVariable('Path', `$new, 'User')"
Write-Host ""
Write-Host "  Then open a new PowerShell window for the change to take effect."
Write-Host ""
