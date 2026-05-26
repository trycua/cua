# windows.ps1 - publish the cua-driver test harness binaries.
#
# Outputs land in libs/cua-driver/rust/test-apps/harness-{wpf,winui3,webview,electron}/
# so the existing sandbox runner picks them up via its mapped folder.
#
# Usage:
#   .\windows.ps1            # build all Windows-runnable apps
#   .\windows.ps1 -Skip wpf  # skip WPF
#   .\windows.ps1 -Skip winui3
#
# Requires: .NET 8 SDK on PATH (Node.js + npm for the Electron app).

param(
    [ValidateSet("none","wpf","winui3","webview","electron")]
    [string]$Skip = "none"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$buildDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$harnessDir = Split-Path -Parent $buildDir
$cuaDriverDir = Split-Path -Parent $harnessDir
$testAppsDir = Join-Path $cuaDriverDir "rust\test-apps"

if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
    Write-Host "[ERROR] dotnet CLI not found on PATH. Install .NET 8 SDK first." -ForegroundColor Red
    exit 1
}

New-Item -ItemType Directory -Force $testAppsDir | Out-Null

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

if ($Skip -ne "wpf") {
    Publish-Project (Join-Path $harnessDir "apps\windows\wpf\CuaTestHarness.Wpf.csproj") "harness-wpf"
}
if ($Skip -ne "winui3") {
    $winuiProj = Join-Path $harnessDir "apps\windows\winui3\CuaTestHarness.WinUI3.csproj"
    if (Test-Path $winuiProj) {
        Publish-Project $winuiProj "harness-winui3"
    } else {
        Write-Host "[SKIP] WinUI3 project not present yet - skipping." -ForegroundColor Yellow
    }
}
if ($Skip -ne "webview") {
    $webProj = Join-Path $harnessDir "apps\windows\webview2\CuaTestHarness.WebView.csproj"
    if (Test-Path $webProj) {
        Publish-Project $webProj "harness-webview"
    } else {
        Write-Host "[SKIP] WebView project not present yet - skipping." -ForegroundColor Yellow
    }
}
if ($Skip -ne "electron") {
    $elecBuild = Join-Path $harnessDir "apps\cross-platform\electron\build.ps1"
    if (Test-Path $elecBuild) {
        Write-Host ""
        Write-Host "[BUILD] electron -> $testAppsDir\harness-electron\" -ForegroundColor Cyan
        # Electron build sets $ErrorActionPreference=Stop internally and
        # throws on npm install / publish failure. Wrap so a throw degrades
        # to a warning rather than aborting the whole harness build.
        try {
            & $elecBuild
            if ($LASTEXITCODE -ne 0) {
                Write-Host "[WARN] Electron build failed (exit $LASTEXITCODE)" -ForegroundColor Yellow
            }
        } catch {
            Write-Host "[WARN] Electron build errored: $($_.Exception.Message)" -ForegroundColor Yellow
        }
    } else {
        Write-Host "[SKIP] Electron project not present yet - skipping." -ForegroundColor Yellow
    }
}

Write-Host ""
Write-Host "[DONE] Test harness build complete." -ForegroundColor Green
