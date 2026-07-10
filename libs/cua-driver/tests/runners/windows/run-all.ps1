# Canonical Windows Rust harness runner.
#
# Run from libs/cua-driver in an interactive RDP/console session:
#   .\tests\runners\windows\run-all.ps1
#   .\tests\runners\windows\run-all.ps1 -RequireGui

param(
    [switch]$NoBuild,
    [switch]$RequireGui
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$runnerDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$runnersDir = Split-Path -Parent $runnerDir
$testsDir = Split-Path -Parent $runnersDir
$cuaDriverRoot = Split-Path -Parent $testsDir
$rustRoot = Join-Path $cuaDriverRoot "rust"
$fixtureBuild = Join-Path $cuaDriverRoot "tests\fixtures\build\windows.ps1"

Write-Host "=== cua-driver Windows Rust run-all ===" -ForegroundColor Cyan
Write-Host "cua-driver root: $cuaDriverRoot"
Write-Host "Rust workspace : $rustRoot"

if ($RequireGui) {
    $env:CUA_REQUIRE_GUI = "1"
    Write-Host "CUA_REQUIRE_GUI=1" -ForegroundColor Yellow
}

$results = New-Object System.Collections.Generic.List[object]

function Add-Result {
    param([string]$Name, [int]$ExitCode)
    $status = if ($ExitCode -eq 0) { "PASS" } else { "FAIL" }
    $results.Add([pscustomobject]@{
        Name = $Name
        Status = $status
        ExitCode = $ExitCode
    }) | Out-Null
}

function Run-Step {
    param(
        [string]$Name,
        [string]$WorkingDirectory,
        [string[]]$CommandArgs
    )

    Write-Host "`n[RUN] $Name" -ForegroundColor Yellow
    Push-Location $WorkingDirectory
    try {
        & cargo @CommandArgs
        $code = if ($null -eq $LASTEXITCODE) { 0 } else { $LASTEXITCODE }
    } finally {
        Pop-Location
    }
    Add-Result $Name $code
    if ($code -ne 0) {
        Write-Host "[FAIL] $Name exited $code" -ForegroundColor Red
    }
}

if (-not $NoBuild) {
    if (-not (Test-Path $fixtureBuild)) {
        throw "Windows fixture build script not found: $fixtureBuild"
    }
    Write-Host "`n[BUILD] Windows fixtures" -ForegroundColor Yellow
    & $fixtureBuild
    if ($LASTEXITCODE -ne 0) {
        Add-Result "windows fixtures" $LASTEXITCODE
        throw "Windows fixture build failed with exit $LASTEXITCODE"
    }
    Add-Result "windows fixtures" 0
}

Run-Step "default Rust tests" $rustRoot @(
    "test", "-p", "cua-driver", "-p", "platform-windows", "--", "--nocapture"
)
Run-Step "guard UX" $rustRoot @(
    "test", "-p", "cua-driver", "--test", "guard_ux_test", "--", "--nocapture", "--test-threads=1"
)
Run-Step "WPF harness" $rustRoot @(
    "test", "-p", "cua-driver", "--test", "harness_wpf_test", "--", "--ignored", "--nocapture", "--test-threads=1"
)
Run-Step "WinUI3 harness" $rustRoot @(
    "test", "-p", "cua-driver", "--test", "harness_winui3_test", "--", "--ignored", "--nocapture", "--test-threads=1"
)
Run-Step "WebView2/Electron harness" $rustRoot @(
    "test", "-p", "cua-driver", "--test", "harness_web_test", "--", "--ignored", "--nocapture", "--test-threads=1"
)
Run-Step "Windows modality input e2e" $rustRoot @(
    "test", "-p", "cua-driver", "--test", "modality_input_e2e_test", "--", "--ignored", "--nocapture", "--test-threads=1"
)

Write-Host "`n=== Summary ===" -ForegroundColor Cyan
$failed = 0
foreach ($r in $results) {
    $color = if ($r.Status -eq "PASS") { "Green" } else { "Red" }
    Write-Host ("{0,-32} {1} ({2})" -f $r.Name, $r.Status, $r.ExitCode) -ForegroundColor $color
    if ($r.ExitCode -ne 0) { $failed++ }
}

if ($failed -gt 0) {
    exit 1
}
