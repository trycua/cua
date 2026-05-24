# CuaDriverInstall.psm1 - shared helpers for install.ps1 + install-local.ps1.
#
# Both scripts import this module to avoid drift in the daemon-kill logic.
#
#   * install-local.ps1 runs from a checked-out repo, so it imports the
#     module from disk via $PSScriptRoot/CuaDriverInstall.psm1.
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

# Prints a clear hint when Stop-CuaDriverDaemons leaves something
# running (almost always a High-IL daemon spawned by the
# RunLevel=Highest autostart task - Medium-IL kill returns Access
# Denied for those).
function Show-CuaDriverDaemonSurvivors {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][array]$Survivors)
    if (-not $Survivors -or $Survivors.Count -eq 0) { return }
    $pids = ($Survivors | ForEach-Object { $_.Id }) -join ', '
    Write-Host "Note: $($Survivors.Count) cua-driver process(es) still running after best-effort kill (pid: $pids)." -ForegroundColor Yellow
    Write-Host "      They are likely High-IL (spawned by RunLevel=Highest autostart task)." -ForegroundColor Yellow
    Write-Host "      From an elevated PowerShell:" -ForegroundColor Yellow
    Write-Host "        taskkill /IM cua-driver.exe /F" -ForegroundColor Yellow
    Write-Host "      Or just reboot. Until they exit, the OLD binary keeps running." -ForegroundColor Yellow
}

# Load this module from disk if `$LocalDir/CuaDriverInstall.psm1` exists
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
        $localPsm = Join-Path $LocalDir "CuaDriverInstall.psm1"
        if (Test-Path -LiteralPath $localPsm) {
            Import-Module -Name $localPsm -Force -ErrorAction Stop
            return
        }
    }
    if (-not $Url) {
        throw "Import-CuaDriverInstallModule: no local file and no -Url to fetch from."
    }
    $body = Invoke-RestMethod -Uri $Url -UseBasicParsing
    $tmp = Join-Path $env:TEMP ("CuaDriverInstall-" + [Guid]::NewGuid().ToString('N') + ".psm1")
    Set-Content -LiteralPath $tmp -Value $body -Encoding UTF8
    try {
        Import-Module -Name $tmp -Force -ErrorAction Stop
    } finally {
        # Module is loaded in memory; the file on disk is no longer
        # needed. Best-effort delete - harmless if it survives.
        Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    }
}

Export-ModuleMember -Function `
    Stop-CuaDriverDaemons, `
    Show-CuaDriverDaemonSurvivors, `
    Import-CuaDriverInstallModule
