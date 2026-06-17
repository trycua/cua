# _install-common.psm1 - shared helpers for install.ps1 + install-local.ps1.
#
# Both scripts import this module to avoid drift in the daemon-kill logic.
#
#   * install-local.ps1 runs from a checked-out repo, so it imports the
#     module from disk via $PSScriptRoot/_install-common.psm1.
#   * install.ps1 is fetched via `irm | iex` and has no file on disk
#     during execution. It uses Import-CuaDriverInstallModule (below)
#     which prefers the on-disk copy when available (dev / CI) and
#     falls back to fetching the .psm1 from GitHub raw.
#
# Keep this module narrow on purpose - it's loaded over the network in
# the production install path, so every additional line is paid for in
# install latency. Things that DO belong here: kill / wait / probe
# helpers that both scripts genuinely need. Things that DON'T: anything
# that's only used by install-local (it can stay in install-local.ps1
# directly) or anything that pulls in a heavy module dependency.

Set-StrictMode -Version Latest

function Resolve-CuaDriverTempDir {
    $candidates = @()
    if ($env:TEMP) { $candidates += $env:TEMP }
    if ($env:TMP) { $candidates += $env:TMP }

    $dotNetTemp = $null
    try { $dotNetTemp = [System.IO.Path]::GetTempPath() } catch {}
    if ($dotNetTemp) { $candidates += $dotNetTemp }

    if ($env:LOCALAPPDATA) { $candidates += (Join-Path $env:LOCALAPPDATA "Temp") }
    if ($env:SystemRoot) { $candidates += (Join-Path $env:SystemRoot "Temp") }

    $seen = @{}
    foreach ($candidate in $candidates) {
        if ([string]::IsNullOrWhiteSpace($candidate)) { continue }

        try {
            $dir = [System.IO.Path]::GetFullPath([string]$candidate)
        } catch {
            continue
        }

        $key = $dir.TrimEnd('\').ToLowerInvariant()
        if ($seen.ContainsKey($key)) { continue }
        $seen[$key] = $true

        if (-not (Test-Path -LiteralPath $dir -PathType Container)) { continue }

        $probe = Join-Path $dir ("cua-driver-temp-probe-" + [Guid]::NewGuid().ToString('N') + ".tmp")
        $stream = $null
        try {
            $stream = [System.IO.File]::Open(
                $probe,
                [System.IO.FileMode]::CreateNew,
                [System.IO.FileAccess]::Write,
                [System.IO.FileShare]::None)
            $stream.Dispose()
            $stream = $null
            Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue
            return $dir
        } catch {
            if ($stream) {
                $stream.Dispose()
            }
            Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue
        }
    }

    throw "could not find a writable temporary directory. Checked TEMP, TMP, .NET temp, LOCALAPPDATA\Temp, and SystemRoot\Temp."
}

# Best-effort kill of any running cua-driver / cua-driver-uia processes
# so the next `cua-driver autostart kick` / `cua-driver mcp` starts the
# FRESH binary, not whatever's still in memory. Without this the
# previous daemon keeps running (and keeps drawing its overlay window)
# until the user reboots - which surfaces as "the bug I just fixed is
# still there" because the in-memory code is pre-fix.
#
# Layers of escalation:
#   1. schtasks /End - terminates an autostart-task instance. Task
#      Scheduler runs as SYSTEM so it can kill High-IL processes that
#      a Medium-IL shell can't. /End on an instance the current user
#      registered does NOT need admin.
#   2. taskkill /F /IM - Medium-IL backstop for any process that
#      wasn't task-attached.
#   3. Returns the surviving process list so callers can warn the user
#      (these are the processes a Medium-IL shell genuinely can't reach
#      - High-IL daemons whose parent wasn't `cua-driver-serve`).
function Stop-CuaDriverDaemons {
    [CmdletBinding()]
    param()
    $prevEAP = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & schtasks.exe /End /TN "cua-driver-serve" 2>$null | Out-Null
        Start-Sleep -Milliseconds 200
        & taskkill.exe /F /IM "cua-driver.exe" /T 2>$null | Out-Null
        & taskkill.exe /F /IM "cua-driver-uia.exe" /T 2>$null | Out-Null
    } finally {
        $ErrorActionPreference = $prevEAP
    }
    Start-Sleep -Milliseconds 200
    return @(Get-Process -Name "cua-driver","cua-driver-uia" -ErrorAction SilentlyContinue)
}

# Probe whether `\\.\pipe\cua-driver` is currently accepting connections.
# Distinguishes a *healthy* High-IL daemon (process alive AND pipe
# responsive - install can proceed by deferring to the user to restart
# from elevated PS) from a *stale* daemon (process alive but pipe gone
# - daemon process is hung, no new daemon can bind, MCP is broken until
# someone kills the zombie).
#
# Returns $true iff a real serve daemon is listening. Uses a short
# 200 ms timeout so install latency stays bounded.
function Test-CuaDriverPipeAlive {
    [CmdletBinding()]
    param()
    try {
        $fs = [System.IO.File]::Open(
            '\\.\pipe\cua-driver',
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::ReadWrite,
            [System.IO.FileShare]::ReadWrite
        )
        $fs.Close()
        return $true
    } catch {
        return $false
    }
}

# Stop-CuaDriverDaemons + stale-daemon detection in one go. Returns a
# PSCustomObject with both the survivor list AND a `Stale` boolean
# flagging "processes are alive but the named pipe is dead". Used by
# install/install-local to swap the user-facing message from "you need
# to kill these from elevated PS" to the stronger "your daemon is
# WEDGED - kill it now or reboot, MCP is currently broken".
function Stop-CuaDriverDaemonsWithHealth {
    [CmdletBinding()]
    param()
    $survivors = Stop-CuaDriverDaemons
    $pipeAlive = Test-CuaDriverPipeAlive
    # @() forces array context so .Count works even when PowerShell
    # collapsed a 1-element array to a single PSObject (strict-mode
    # otherwise errors on .Count missing on the bare object).
    $count = @($survivors).Count
    $stale = ($count -gt 0 -and -not $pipeAlive)
    return [pscustomobject]@{
        Survivors  = $survivors
        PipeAlive  = $pipeAlive
        Stale      = $stale
    }
}

# Prints a clear hint when Stop-CuaDriverDaemons leaves something
# running (almost always a High-IL daemon spawned by the
# RunLevel=Highest autostart task - Medium-IL kill returns Access
# Denied for those).
function Show-CuaDriverDaemonSurvivors {
    [CmdletBinding()]
    # AllowNull + non-mandatory: PowerShell collapses `return @()` from
    # Stop-CuaDriverDaemons to $null at the call site, which would fail
    # a `Mandatory = $true [array]` bind. The body below already treats
    # null and empty array as the no-survivors case, so accept both.
    #
    # `-Stale` toggles the user-facing wording from "the old binary is
    # still running" (mild - install completed, OLD binary in memory
    # until the daemon restarts) to "MCP is currently broken because
    # the daemon process is alive but its named pipe is dead" (loud -
    # nothing will work until the zombie process is killed).
    param(
        [Parameter()][AllowNull()][array]$Survivors,
        [switch]$Stale
    )
    if (-not $Survivors -or $Survivors.Count -eq 0) { return }
    $pids = ($Survivors | ForEach-Object { $_.Id }) -join ', '
    if ($Stale) {
        Write-Host "" -ForegroundColor Red
        Write-Host "ERROR: cua-driver daemon is STALE - process alive (pid: $pids) but \\.\pipe\cua-driver" -ForegroundColor Red
        Write-Host "       is not accepting connections. The daemon is wedged; MCP / CLI calls will fail" -ForegroundColor Red
        Write-Host "       with 'cannot find the file specified' until this process is killed." -ForegroundColor Red
        Write-Host "" -ForegroundColor Red
        Write-Host "       From an ELEVATED PowerShell (right-click PowerShell, 'Run as Administrator'):" -ForegroundColor Yellow
        Write-Host "         Stop-Process -Id $pids -Force" -ForegroundColor Yellow
        Write-Host "         schtasks /Run /TN 'cua-driver-serve'" -ForegroundColor Yellow
        Write-Host "" -ForegroundColor Yellow
        Write-Host "       Reboot also clears it. After the kill+restart, MCP recovers automatically." -ForegroundColor Yellow
    } else {
        Write-Host "Note: $($Survivors.Count) cua-driver process(es) still running after best-effort kill (pid: $pids)." -ForegroundColor Yellow
        Write-Host "      They are likely High-IL (spawned by RunLevel=Highest autostart task)." -ForegroundColor Yellow
        Write-Host "      From an elevated PowerShell:" -ForegroundColor Yellow
        Write-Host "        taskkill /IM cua-driver.exe /F" -ForegroundColor Yellow
        Write-Host "      Or just reboot. Until they exit, the OLD binary keeps running." -ForegroundColor Yellow
    }
}

# Load this module from disk if `$LocalDir/_install-common.psm1` exists
# (install-local.ps1 / checked-out install.ps1), else fetch the same
# file from GitHub raw and load it as an in-memory module
# (`irm | iex` install.ps1 path).
#
# Either way the caller ends up with Stop-CuaDriverDaemons +
# Show-CuaDriverDaemonSurvivors in scope.
#
# Pulled into the module itself (recursively used) so install.ps1's
# inline bootstrap is one short call. Callers must define
# $CuaDriverInstallPsmUrl before invoking the fallback branch.
function Import-CuaDriverInstallModule {
    [CmdletBinding()]
    param(
        [string]$LocalDir,
        [string]$Url
    )
    if ($LocalDir) {
        $localPsm = Join-Path $LocalDir "_install-common.psm1"
        if (Test-Path -LiteralPath $localPsm) {
            Import-Module -Name $localPsm -Force -ErrorAction Stop
            return
        }
    }
    if (-not $Url) {
        throw "Import-CuaDriverInstallModule: no local file and no -Url to fetch from."
    }
    $body = Invoke-RestMethod -Uri $Url -UseBasicParsing
    $tmp = Join-Path (Resolve-CuaDriverTempDir) ("CuaDriverInstall-" + [Guid]::NewGuid().ToString('N') + ".psm1")
    Set-Content -LiteralPath $tmp -Value $body -Encoding UTF8
    try {
        Import-Module -Name $tmp -Force -ErrorAction Stop
    } finally {
        # Module is loaded in memory; the file on disk is no longer
        # needed. Best-effort delete - harmless if it survives.
        Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    }
}

# Trigger UAC and spawn a brief elevated PowerShell that kills the stale
# (High-IL) cua-driver pids and restarts the scheduled task. Returns
# $true iff the elevated helper actually killed everything AND the
# named pipe is alive again afterward. Returns $false on UAC cancel,
# elevation failure, or post-recovery health-check still failing.
#
# Why a helper rather than self-elevating the whole install: the
# install only needs admin to break the wedge. Binary copy, PATH
# munging, scheduled-task re-registration (which has its own UAC
# prompt anyway) all run fine at Medium IL. Asking for admin just
# for those steps would needlessly broaden the prompt's blast
# radius.
#
# The helper is a single short -Command string so we don't need a
# temp file. UAC users see "Windows PowerShell" as the requesting
# app, with the command in the elevation prompt's "Show details"
# panel.
function Invoke-CuaDriverStaleDaemonKill {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][int[]]$Pids
    )
    if (-not $Pids -or $Pids.Count -eq 0) { return $true }
    $pidList = ($Pids -join ',')
    # Build the elevated payload:
    #   1. Stop-Process each pid, force, swallow errors (process already gone is OK).
    #   2. Re-run the scheduled task so a fresh daemon binds the pipe.
    #   3. Brief wait + verify the pipe is reachable.
    # The exit code from the helper signals success (0) / pipe still
    # dead post-restart (3) so the install-side caller can tell the
    # user whether to expect MCP to work.
    $payload = @"
`$ErrorActionPreference = 'Continue'
foreach (`$p in @($pidList)) {
    try { Stop-Process -Id `$p -Force -ErrorAction Stop } catch {}
}
& schtasks.exe /Run /TN 'cua-driver-serve' | Out-Null
Start-Sleep -Milliseconds 1500
try {
    `$fs = [System.IO.File]::Open('\\.\pipe\cua-driver','Open','ReadWrite','ReadWrite')
    `$fs.Close()
    exit 0
} catch {
    exit 3
}
"@
    try {
        $proc = Start-Process `
            -FilePath powershell.exe `
            -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', $payload) `
            -Verb RunAs `
            -PassThru `
            -Wait `
            -WindowStyle Hidden `
            -ErrorAction Stop
        return ($proc.ExitCode -eq 0)
    } catch {
        # UAC cancel surfaces as System.ComponentModel.Win32Exception ("The
        # operation was canceled by the user"). Other failures (no
        # interactive desktop, etc.) take the same path. Return false so
        # the caller falls back to the printed instructions.
        return $false
    }
}

# Convenience wrapper: detect stale state, if stale prompt the user, on
# yes invoke the elevated kill, re-probe, report. Either way return the
# post-recovery state so the install script can decide whether to
# proceed quietly or print the manual-recovery instructions.
#
# Designed to be a drop-in replacement for the
# `Stop-CuaDriverDaemonsWithHealth + Show-CuaDriverDaemonSurvivors`
# pair at the call site.
function Repair-CuaDriverStaleDaemon {
    [CmdletBinding()]
    param(
        # Default-true so install scripts auto-prompt. Pass `-AutoConfirm`
        # to skip the y/n prompt (CI / automated runs) — UAC still prompts
        # for elevation itself.
        [switch]$AutoConfirm
    )
    $result = Stop-CuaDriverDaemonsWithHealth
    if (-not $result.Stale) {
        Show-CuaDriverDaemonSurvivors -Survivors $result.Survivors
        return $result
    }
    # Stale: tell user what we see, offer to elevate.
    $survivorPids = @($result.Survivors | ForEach-Object { $_.Id })
    Write-Host ""
    Write-Host "Detected STALE cua-driver daemon (pid: $($survivorPids -join ', '))." -ForegroundColor Yellow
    Write-Host "  Process is alive but \\.\pipe\cua-driver is not accepting connections." -ForegroundColor Yellow
    Write-Host "  Fixing this requires admin to terminate the High-IL daemon process." -ForegroundColor Yellow
    Write-Host ""

    $proceed = $AutoConfirm
    if (-not $proceed) {
        # Single-key prompt; default Y on Enter. Y/yes/[Enter] -> elevate,
        # anything else -> skip elevation and fall through to manual-recovery
        # instructions.
        $ans = Read-Host "  Trigger UAC prompt to kill the stale daemon now? [Y/n]"
        $proceed = ($ans -eq '' -or $ans -match '^[Yy]')
    }

    if (-not $proceed) {
        Show-CuaDriverDaemonSurvivors -Survivors $result.Survivors -Stale
        return $result
    }

    Write-Host "  Triggering UAC prompt (accept to kill the stale daemon)..." -ForegroundColor Cyan
    $ok = Invoke-CuaDriverStaleDaemonKill -Pids $survivorPids
    if ($ok) {
        Write-Host "  Stale daemon killed; new cua-driver-serve started; pipe is healthy." -ForegroundColor Green
        return [pscustomobject]@{
            Survivors = @()
            PipeAlive = $true
            Stale     = $false
        }
    } else {
        Write-Host "  Elevated kill did not complete (UAC cancelled or pipe still dead)." -ForegroundColor Red
        # Re-probe; if the user accepted UAC but the daemon restart
        # failed, the survivors list may have shrunk to just the
        # un-killable processes — re-show with current state.
        $rep = Stop-CuaDriverDaemonsWithHealth
        Show-CuaDriverDaemonSurvivors -Survivors $rep.Survivors -Stale:$rep.Stale
        return $rep
    }
}

Export-ModuleMember -Function `
    Stop-CuaDriverDaemons, `
    Stop-CuaDriverDaemonsWithHealth, `
    Test-CuaDriverPipeAlive, `
    Show-CuaDriverDaemonSurvivors, `
    Invoke-CuaDriverStaleDaemonKill, `
    Repair-CuaDriverStaleDaemon, `
    Import-CuaDriverInstallModule
