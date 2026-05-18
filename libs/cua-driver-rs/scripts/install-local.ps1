# cua-driver-rs local/debug installer (Windows). Builds from the current
# source tree and drops the resulting cua-driver.exe into the same
# install layout that scripts/install.ps1 produces — so a local build
# and a release install can coexist + the `current` junction can flip
# between them.
#
# Mirrors libs/cua-driver/scripts/install-local.sh (Swift) in shape:
#   -Release    build the release configuration (default: debug)
#   -AutoStart  register the cua-driver-serve Scheduled Task at logon
#               (Windows-native equivalent of macOS LaunchAgent). Default
#               off; the post-install message prints the registration
#               recipe so you can opt in later.
#
# Not for end-users — `irm https://.../install.ps1 | iex` fetches a
# signed/built release from GitHub. This script is for the developer
# loop (rapid edit/build/test on a Windows host).
#
# Layout produced (matches install.ps1 — see its header for details):
#
#   <visibleBinDir>            [junction → currentDir]
#   <currentDir>               [junction → release dir, retargeted here]
#   <release dir>              [real dir, this script's output]
#     <version>-local-<config>-<target>\cua-driver.exe
#
# The version-string carries `-local-debug` / `-local-release` so it
# never collides with a real release dir and is trivial to garbage-collect.

[CmdletBinding()]
param(
    [switch]$Release,
    [switch]$AutoStart
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# Reuse the production installer's helpers (path resolution, junction
# wiring, autostart registration) by dot-sourcing the relevant bits via
# a small wrapper. install.ps1 expects to run end-to-end, so we don't
# dot-source the whole thing — instead duplicate the small handful of
# operations we need, calling out matching install.ps1 functions where
# the logic would otherwise drift.

$ScriptDir   = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot    = (Resolve-Path "$ScriptDir\..").Path
$BinaryName  = "cua-driver.exe"
$Config      = if ($Release) { "release" } else { "debug" }
$Target      = if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') {
    "aarch64-pc-windows-msvc"
} else {
    "x86_64-pc-windows-msvc"
}

# ---------- Paths (must match install.ps1's defaults) ----------------------

if ($env:CUA_DRIVER_RS_INSTALL_DIR) {
    $VisibleBinDir = $env:CUA_DRIVER_RS_INSTALL_DIR
} else {
    $VisibleBinDir = Join-Path $env:LOCALAPPDATA "Programs\trycua\cua-driver-rs\bin"
}
if ($env:CUA_DRIVER_RS_HOME) {
    $PackageHome = $env:CUA_DRIVER_RS_HOME
} else {
    $PackageHome = Join-Path $env:USERPROFILE ".cua-driver-rs"
}
$CurrentDir  = Join-Path $PackageHome "packages\current"
$ReleasesDir = Join-Path $PackageHome "packages\releases"

# ---------- Helpers (lifted from install.ps1) -----------------------------

. "$ScriptDir\install.ps1.helpers.ps1" -ErrorAction SilentlyContinue 2>$null
# install.ps1 doesn't currently extract its helpers into a separate
# .helpers.ps1 — fall back to defining the absolute minimum here. If a
# helpers file appears later, the dot-source above wins and these are
# never compiled.
if (-not (Get-Command -Name 'Test-IsJunction' -ErrorAction SilentlyContinue)) {

    function Test-IsJunction([string]$path) {
        if (-not (Test-Path -LiteralPath $path)) { return $false }
        $item = Get-Item -LiteralPath $path -Force
        return [bool]($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)
    }

    function Ensure-Junction([string]$linkPath, [string]$targetPath) {
        New-Item -ItemType Directory -Path (Split-Path -Parent $linkPath) -Force | Out-Null
        if (Test-Path -LiteralPath $linkPath) {
            if (Test-IsJunction $linkPath) {
                # Always retarget — that's the point of this helper.
                cmd /c rmdir (Resolve-Path -LiteralPath $linkPath).Path | Out-Null
            } else {
                throw "Refusing to replace non-junction at $linkPath. Move or delete it first."
            }
        }
        cmd /c mklink /J $linkPath $targetPath | Out-Null
    }

    function Register-CuaDriverAutostart {
        param([Parameter(Mandatory = $true)][string]$InstalledBinary)
        if (-not (Test-Path -LiteralPath $InstalledBinary)) {
            throw "binary not found at $InstalledBinary"
        }
        $user = "$env:COMPUTERNAME\$env:USERNAME"
        $action = New-ScheduledTaskAction -Execute $InstalledBinary -Argument 'serve' -WorkingDirectory $env:USERPROFILE
        $trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
        $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
        $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit (New-TimeSpan -Hours 0)
        Unregister-ScheduledTask -TaskName 'cua-driver-serve' -Confirm:$false -ErrorAction SilentlyContinue
        Register-ScheduledTask -TaskName 'cua-driver-serve' -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Description 'cua-driver-rs: serve daemon, auto-start at interactive logon' | Out-Null
    }
}

function Write-Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }

# ---------- Prerequisites --------------------------------------------------

Write-Step "cua-driver-rs local installer (Windows)"
Write-Host "  source:    $RepoRoot"
Write-Host "  config:    $Config"
Write-Host "  target:    $Target"
Write-Host "  visible:   $VisibleBinDir"
Write-Host "  current:   $CurrentDir"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Error: cargo not found on PATH." -ForegroundColor Red
    Write-Host "Install Rust + MSVC toolchain via rustup-init: https://rustup.rs/"
    exit 1
}

# ---------- Build ----------------------------------------------------------

Write-Step "cargo build ($Config) -p cua-driver"
Push-Location $RepoRoot
try {
    if ($Release) {
        & cargo build --release -p cua-driver
    } else {
        & cargo build -p cua-driver
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Error: cargo build failed." -ForegroundColor Red
        exit $LASTEXITCODE
    }
}
finally {
    Pop-Location
}
$BuiltBinary = Join-Path $RepoRoot "target\$Config\$BinaryName"
if (-not (Test-Path -LiteralPath $BuiltBinary)) {
    Write-Host "Error: build produced no binary at $BuiltBinary" -ForegroundColor Red
    exit 1
}

# ---------- Stage into versioned release dir -------------------------------

$VersionTag  = "0.0.0-local-$Config"
$VersionedDir = Join-Path $ReleasesDir "$VersionTag-$Target"
Write-Step "staging into $VersionedDir"
New-Item -ItemType Directory -Path $VersionedDir -Force | Out-Null
Copy-Item -LiteralPath $BuiltBinary -Destination (Join-Path $VersionedDir $BinaryName) -Force
$installedBinary = Join-Path $VersionedDir $BinaryName

# ---------- Repoint junctions ---------------------------------------------

Write-Step "retargeting $CurrentDir -> $VersionedDir"
Ensure-Junction -linkPath $CurrentDir -targetPath $VersionedDir

Write-Step "ensuring $VisibleBinDir -> $CurrentDir"
if (Test-Path -LiteralPath $VisibleBinDir) {
    if (-not (Test-IsJunction $VisibleBinDir)) {
        Write-Host "$VisibleBinDir exists and is not a junction; aborting." -ForegroundColor Red
        Write-Host "Remove or relocate it, then re-run."
        exit 1
    }
}
Ensure-Junction -linkPath $VisibleBinDir -targetPath $CurrentDir

# ---------- Done -----------------------------------------------------------

Write-Host ""
Write-Step "installed"
Write-Host "  exe:    $(Join-Path $VisibleBinDir $BinaryName)"
Write-Host "  source: $installedBinary"
Write-Host ""

if ($AutoStart) {
    Write-Step "registering Scheduled Task 'cua-driver-serve'"
    try {
        Register-CuaDriverAutostart -InstalledBinary (Join-Path $VisibleBinDir $BinaryName)
        Write-Host "  Registered. cua-driver serve auto-starts at every interactive logon." -ForegroundColor Green
        Write-Host "  Start now without re-logging: schtasks /Run /TN cua-driver-serve"
        Write-Host "  Remove:                       schtasks /Delete /TN cua-driver-serve /F"
    }
    catch {
        Write-Host "  Failed to register: $($_.Exception.Message)" -ForegroundColor Red
    }
} else {
    Write-Host "Auto-start (optional): re-run with -AutoStart to register a logon Scheduled Task,"
    Write-Host "  or use 'schtasks /Run /TN cua-driver-serve' if you registered it previously."
}
Write-Host ""
