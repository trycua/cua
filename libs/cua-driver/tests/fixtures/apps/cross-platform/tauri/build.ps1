# build.ps1 - stage the Tauri test harness for cua-driver tests on Windows.
#
# Output: libs/cua-driver/rust/test-apps/harness-tauri/CuaTestHarness.Tauri.exe

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$tauriDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$crossDir = Split-Path -Parent $tauriDir
$appsDir = Split-Path -Parent $crossDir
$harnessDir = Split-Path -Parent $appsDir
$testsDir = Split-Path -Parent $harnessDir
$cuaDriverDir = Split-Path -Parent $testsDir
$testAppsDir = Join-Path $cuaDriverDir "rust\test-apps"
$outDir = Join-Path $testAppsDir "harness-tauri"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "[ERROR] cargo not on PATH. Install Rust first." -ForegroundColor Red
    exit 1
}

$webDir = Join-Path $tauriDir "web"
if (-not (Test-Path $webDir)) { New-Item -ItemType Directory $webDir | Out-Null }
Copy-Item (Join-Path $harnessDir "shared\web\*") $webDir -Recurse -Force

$manifest = Join-Path $tauriDir "src-tauri\Cargo.toml"
Write-Host "[BUILD] cargo build --release --manifest-path $manifest" -ForegroundColor Cyan
$oldTargetDir = $env:CARGO_TARGET_DIR
$env:CARGO_TARGET_DIR = Join-Path $tauriDir "src-tauri\target"
try {
    & cargo build --release --manifest-path $manifest
} finally {
    $env:CARGO_TARGET_DIR = $oldTargetDir
}
if ($LASTEXITCODE -ne 0) { throw "cargo build failed for Tauri harness" }

$srcExe = Join-Path $tauriDir "src-tauri\target\release\cua-test-harness-tauri.exe"
if (-not (Test-Path $srcExe)) {
    throw "Tauri exe not found after build: $srcExe"
}

if (Test-Path $outDir) { Remove-Item $outDir -Recurse -Force }
New-Item -ItemType Directory -Force $outDir | Out-Null
Copy-Item $srcExe (Join-Path $outDir "CuaTestHarness.Tauri.exe") -Force
Write-Host "[OK]    Staged: $outDir\CuaTestHarness.Tauri.exe" -ForegroundColor Green
