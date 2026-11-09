# windows.ps1 - publish the cua-driver test fixture binaries.
#
# Outputs land in libs/cua-driver/rust/test-apps/harness-{wpf,winui3,webview,electron,tauri}/
# so the existing sandbox runner picks them up via its mapped folder.
#
# Usage:
#   .\windows.ps1            # build all Windows-runnable apps
#   .\windows.ps1 -Skip wpf  # skip WPF
#   .\windows.ps1 -Skip winui3
#
# Requires: .NET 8 SDK on PATH (Node.js + npm for Electron, Rust for Tauri).

param(
    [ValidateSet("none","wpf","winui3","webview","electron","tauri")]
    [string]$Skip = "none",
    [ValidateSet("wpf","winui3","webview","electron","tauri")]
    [string[]]$Targets = @("wpf","winui3","webview","electron","tauri")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$buildDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$harnessDir = Split-Path -Parent $buildDir
$testsDir = Split-Path -Parent $harnessDir
$cuaDriverDir = Split-Path -Parent $testsDir
$testAppsDir = Join-Path $cuaDriverDir "rust\test-apps"

if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
    Write-Host "[ERROR] dotnet CLI not found on PATH. Install .NET 8 SDK first." -ForegroundColor Red
    exit 1
}

New-Item -ItemType Directory -Force $testAppsDir | Out-Null

function Should-Build {
    param([string]$Name)
    return $Skip -ne $Name -and $Targets -contains $Name
}

function Publish-Project {
    param([string]$ProjPath, [string]$OutDirName)
    $outDir = Join-Path $testAppsDir $OutDirName
    Write-Host ""
    Write-Host "[BUILD] $ProjPath -> $outDir" -ForegroundColor Cyan
    # Self-contained so the published exe runs in the Windows Sandbox without
    # requiring a separate .NET runtime install. The harness is small (~30MB
    # per project after framework-dependent stripping), so the size cost is
    # acceptable for the deterministic-test gain.
    $publishArgs = @(
        "publish", $ProjPath,
        "-c", "Release",
        "-r", "win-x64",
        "--self-contained", "true",
        "-o", $outDir,
        "-p:PublishSingleFile=false",
        "-p:DebugType=embedded"
    )
    & dotnet @publishArgs
    if ($LASTEXITCODE -ne 0) { throw "dotnet publish failed for $ProjPath" }
    Write-Host "[OK]    Published: $outDir" -ForegroundColor Green
}

if (Should-Build "wpf") {
    Publish-Project (Join-Path $harnessDir "apps\windows\wpf\CuaTestHarness.Wpf.csproj") "harness-wpf"
}
if (Should-Build "winui3") {
    $winuiProj = Join-Path $harnessDir "apps\windows\winui3\CuaTestHarness.WinUI3.csproj"
    if (Test-Path $winuiProj) {
        Publish-Project $winuiProj "harness-winui3"
    } else {
        Write-Host "[SKIP] WinUI3 project not present yet - skipping." -ForegroundColor Yellow
    }
}
if (Should-Build "webview") {
    $webProj = Join-Path $harnessDir "apps\windows\webview2\CuaTestHarness.WebView.csproj"
    if (Test-Path $webProj) {
        Publish-Project $webProj "harness-webview"
    } else {
        Write-Host "[SKIP] WebView project not present yet - skipping." -ForegroundColor Yellow
    }
}
if (Should-Build "electron") {
    $elecBuild = Join-Path $harnessDir "apps\cross-platform\electron\build.ps1"
    if (Test-Path $elecBuild) {
        Write-Host ""
        Write-Host "[BUILD] electron -> $testAppsDir\harness-electron\" -ForegroundColor Cyan
        & $elecBuild
        if ($LASTEXITCODE -ne 0) { throw "Electron harness build failed" }
    } else {
        throw "Electron build script not found: $elecBuild"
    }
}
if (Should-Build "tauri") {
    $tauriBuild = Join-Path $harnessDir "apps\cross-platform\tauri\build.ps1"
    if (Test-Path $tauriBuild) {
        Write-Host ""
        Write-Host "[BUILD] tauri -> $testAppsDir\harness-tauri\" -ForegroundColor Cyan
        & $tauriBuild
        if ($LASTEXITCODE -ne 0) { throw "Tauri harness build failed" }
    } else {
        throw "Tauri build script not found: $tauriBuild"
    }
}

Write-Host ""
Write-Host "[DONE] Test harness build complete." -ForegroundColor Green
