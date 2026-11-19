# Read-only readiness checks before starting the Windows desktop matrix.
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$repoRoot = (Resolve-Path (Join-Path $scriptDir "..\..\..")).Path
$rustRoot = Join-Path $repoRoot "libs\cua-driver\rust"
$driverBin = if ($env:CUA_TEST_DRIVER_BIN) { $env:CUA_TEST_DRIVER_BIN } else {
    Join-Path $rustRoot "target\release\cua-driver.exe"
}
$failures = 0

function Ready([string]$message) { Write-Host "[ready] $message" }
function Optional([string]$message) { Write-Host "[optional] $message" }
function Missing([string]$message) {
    Write-Host "[missing] $message" -ForegroundColor Red
    $script:failures++
}

if (-not [Environment]::Is64BitOperatingSystem) {
    Missing "64-bit Windows host required"
} else {
    Ready "64-bit Windows; process architecture $([System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture)"
}

try {
    & (Join-Path $scriptDir "verify-user-session.ps1")
    Ready "interactive input desktop"
} catch {
    Missing "interactive input desktop unavailable: $($_.Exception.Message)"
}

foreach ($tool in @("ffmpeg.exe", "ffprobe.exe")) {
    if (Get-Command $tool -ErrorAction SilentlyContinue) {
        Ready "$tool available"
    } else {
        Missing "$tool required for E2E recording"
    }
}

$sourceSha = $null
if (Get-Command git -ErrorAction SilentlyContinue) {
    $sourceSha = (& git -C $repoRoot rev-parse HEAD 2>$null)
}
if ($sourceSha -match '^[0-9a-fA-F]{40}$') {
    Ready "checked-out source SHA: $sourceSha"
} else {
    Missing "checked-out source SHA unavailable"
}
if ($env:CUA_E2E_SOURCE_SHA -and $sourceSha -ine $env:CUA_E2E_SOURCE_SHA) {
    Missing "CUA_E2E_SOURCE_SHA differs from the checkout; sync exact source before testing"
}

if (Test-Path -LiteralPath $driverBin -PathType Leaf) {
    try {
        $version = & $driverBin --version 2>&1
        if ($LASTEXITCODE -eq 0) { Ready "driver version: $version ($driverBin); source identity not yet verified" }
        else { Missing "source driver cannot run --version: $driverBin" }
    } catch {
        Missing "source driver cannot run --version: $driverBin ($($_.Exception.Message))"
    }
} else {
    Optional "source driver not built: $driverBin"
}

foreach ($browser in @("chrome.exe", "msedge.exe")) {
    if (Get-Command $browser -ErrorAction SilentlyContinue) {
        Optional "browser available on PATH: $browser"
    }
}
Optional "fixture, UIA, capture, permission, and recording checks require the strict matrix preflight"

if ($failures -gt 0) {
    throw "$failures required preflight check(s) failed"
}
Ready "lightweight environment checks complete (not desktop certification)"
