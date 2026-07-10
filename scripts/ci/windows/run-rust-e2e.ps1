# Run the canonical Rust desktop matrix in an active Windows user session.
# Scenario definitions and assertions stay in the Rust integration test.
param(
    [switch]$NoBuild,
    [ValidateSet("shared", "native", "all")]
    [string]$Suite = "shared",
    [switch]$RequireGui
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$repoRoot = (Resolve-Path (Join-Path $scriptDir "..\..\..")).Path
$driverRoot = Join-Path $repoRoot "libs\cua-driver"
$rustRoot = Join-Path $driverRoot "rust"
$artifactDir = Join-Path $repoRoot "artifacts\cua-driver\windows"
New-Item -ItemType Directory -Force $artifactDir | Out-Null

& (Join-Path $scriptDir "verify-user-session.ps1")
if ($RequireGui) { $env:CUA_REQUIRE_GUI = "1" }

$env:CUA_TEST_WORKSPACE_ROOT = $rustRoot
$env:CUA_TEST_DRIVER_BIN = Join-Path $rustRoot "target\release\cua-driver.exe"
$env:CUA_TEST_APPS_ROOT = Join-Path $rustRoot "test-apps"
$env:CUA_TEST_REQUIRE_FIXTURES = "1"
$env:CUA_TEST_DRIVER_STDERR = "1"

if (-not $NoBuild) {
    & cargo build --release -p cua-driver --manifest-path (Join-Path $rustRoot "Cargo.toml")
    if ($LASTEXITCODE -ne 0) { throw "Rust driver build failed" }
    & (Join-Path $scriptDir "build-harnesses.ps1")
}

if (-not (Test-Path $env:CUA_TEST_DRIVER_BIN)) {
    throw "Driver binary not found: $($env:CUA_TEST_DRIVER_BIN)"
}
foreach ($fixture in @(
    (Join-Path $env:CUA_TEST_APPS_ROOT "harness-electron\CuaTestHarness.Electron.exe"),
    (Join-Path $env:CUA_TEST_APPS_ROOT "harness-tauri\CuaTestHarness.Tauri.exe")
)) {
    if (-not (Test-Path $fixture)) { throw "Required fixture was not built: $fixture" }
}

function Invoke-CargoTest {
    param([string]$Name, [string[]]$Arguments)
    Write-Host "[RUN] $Name" -ForegroundColor Yellow
    Push-Location $rustRoot
    try {
        $logPath = Join-Path $artifactDir ("$($Name -replace '[^A-Za-z0-9_.-]', '-')}.log")
        $output = & cargo @Arguments 2>&1
        $exitCode = $LASTEXITCODE
        $output | Tee-Object -FilePath $logPath
        if ($exitCode -ne 0) { throw "$Name failed with exit code $exitCode" }
    } finally {
        Pop-Location
    }
}

if ($Suite -in @("shared", "all")) {
    Invoke-CargoTest "shared behavior matrix" @(
        "test", "-p", "cua-driver", "--test", "cross_platform_behavior_test", "--",
        "--ignored", "--nocapture", "--test-threads=1"
    )
}

if ($Suite -in @("native", "all")) {
    Invoke-CargoTest "Windows native harnesses" @(
        "test", "-p", "cua-driver", "--test", "harness_wpf_test", "--",
        "--ignored", "--nocapture", "--test-threads=1"
    )
    Invoke-CargoTest "Windows web harnesses" @(
        "test", "-p", "cua-driver", "--test", "harness_web_test", "--",
        "--ignored", "--nocapture", "--test-threads=1"
    )
}

Write-Host "Windows Rust e2e suite completed: $Suite" -ForegroundColor Green
