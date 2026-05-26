# build.ps1 - stage the Electron test harness for cua-driver tests.
#
# electron-builder portable mode needs admin (symlink privilege) for the
# winCodeSign cache extraction. We instead stage a flat folder containing
# the electron runtime + our app resources, with electron.exe renamed to
# CuaTestHarness.Electron.exe so tests get a deterministic exe name.
#
# Output:  ../../../../rust/test-apps/harness-electron/

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$elecDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$crossDir = Split-Path -Parent $elecDir
$appsDir = Split-Path -Parent $crossDir
$harnessDir = Split-Path -Parent $appsDir
$cuaDriverDir = Split-Path -Parent $harnessDir
$testAppsDir = Join-Path $cuaDriverDir "rust\test-apps"
$outDir = Join-Path $testAppsDir "harness-electron"

if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
    Write-Host "[ERROR] npm not on PATH. Install Node.js first." -ForegroundColor Red
    exit 1
}

Push-Location $elecDir
try {
    $webDir = Join-Path $elecDir "web"
    if (-not (Test-Path $webDir)) { New-Item -ItemType Directory $webDir | Out-Null }
    Copy-Item (Join-Path $harnessDir "shared\web\*") $webDir -Recurse -Force

    if (-not (Test-Path "node_modules\electron\dist\electron.exe")) {
        Write-Host "[INSTALL] npm install (first run)..." -ForegroundColor Yellow
        npm install --silent
        if ($LASTEXITCODE -ne 0) { throw "npm install failed" }
    }

    $electronSrc = Join-Path $elecDir "node_modules\electron\dist"
    if (-not (Test-Path (Join-Path $electronSrc "electron.exe"))) {
        throw "electron.exe not found under $electronSrc - npm install incomplete"
    }

    Write-Host "[STAGE] Copying electron runtime + app to $outDir..." -ForegroundColor Cyan
    if (Test-Path $outDir) { Remove-Item $outDir -Recurse -Force }
    Copy-Item $electronSrc $outDir -Recurse -Force

    $appDir = Join-Path $outDir "resources\app"
    if (-not (Test-Path $appDir)) { New-Item -ItemType Directory $appDir -Force | Out-Null }
    Copy-Item (Join-Path $elecDir "main.js")      $appDir -Force
    Copy-Item (Join-Path $elecDir "package.json") $appDir -Force
    Copy-Item $webDir (Join-Path $appDir "web") -Recurse -Force

    Rename-Item (Join-Path $outDir "electron.exe") "CuaTestHarness.Electron.exe" -Force
    Write-Host "[OK]    Staged: $outDir\CuaTestHarness.Electron.exe" -ForegroundColor Green
} finally {
    Pop-Location
}
