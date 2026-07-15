# Run the canonical Rust desktop matrix in an active Windows user session.
# Scenario definitions and assertions stay in the Rust integration test.
param(
    [switch]$NoBuild,
    [switch]$RequireGui
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$repoRoot = (Resolve-Path (Join-Path $scriptDir "..\..\..")).Path
$driverRoot = Join-Path $repoRoot "libs\cua-driver"
$rustRoot = Join-Path $driverRoot "rust"
$suite = if ([string]::IsNullOrWhiteSpace($env:CUA_E2E_INTERNAL_LANE)) { "all" } else { $env:CUA_E2E_INTERNAL_LANE }
if ($suite -notin @("shared", "native", "capture", "all")) {
    throw "Unsupported internal lane: $suite"
}
$artifactDir = Join-Path $repoRoot "artifacts\cua-driver\windows"
New-Item -ItemType Directory -Force $artifactDir | Out-Null
$recordingRoot = Join-Path $artifactDir "recordings"
Remove-Item -Path $recordingRoot -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $recordingRoot | Out-Null
$resultsPath = Join-Path $artifactDir "results.jsonl"
$casesPath = Join-Path $artifactDir "cases.jsonl"
$environmentPath = Join-Path $artifactDir "environment.jsonl"
$summaryPath = Join-Path $artifactDir "summary.md"
foreach ($path in @($casesPath, $environmentPath, $resultsPath)) {
    New-Item -ItemType File -Force $path | Out-Null
    Clear-Content $path
}
Remove-Item -Force -ErrorAction SilentlyContinue $summaryPath
$env:CUA_E2E_DECLARATIONS_FILE = $casesPath
$env:CUA_E2E_ENVIRONMENT_FILE = $environmentPath
$env:CUA_E2E_RESULTS_FILE = $resultsPath
$env:CUA_E2E_RECORDINGS_ROOT = $recordingRoot

$ffmpeg = Get-Command ffmpeg.exe -ErrorAction SilentlyContinue
$ffprobe = Get-Command ffprobe.exe -ErrorAction SilentlyContinue
if ($null -eq $ffmpeg -or $null -eq $ffprobe) {
    throw "FFmpeg and ffprobe are required for E2E trajectory videos"
}

& (Join-Path $scriptDir "verify-user-session.ps1")
if ($RequireGui) { $env:CUA_REQUIRE_GUI = "1" }

$env:CUA_TEST_WORKSPACE_ROOT = $rustRoot
$env:CUA_TEST_DRIVER_BIN = Join-Path $rustRoot "target\release\cua-driver.exe"
$env:CUA_TEST_APPS_ROOT = Join-Path $rustRoot "test-apps"
$env:CUA_TEST_REQUIRE_FIXTURES = "1"
$env:CUA_TEST_DRIVER_STDERR = "1"
$env:CUA_E2E_FORBID_SKIPS = "1"
Remove-Item Env:CUA_E2E_EXPECTED_MIN_CELLS -ErrorAction SilentlyContinue
if (($suite -in @("shared", "all")) -and
    [string]::IsNullOrWhiteSpace($env:CUA_E2E_CELL_FILTER) -and
    [string]::IsNullOrWhiteSpace($env:CUA_E2E_HARNESS_FILTER)) {
    $env:CUA_E2E_EXPECTED_MIN_CELLS = "80"
}

if (-not $NoBuild) {
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & cargo build --release -p cua-driver --manifest-path (Join-Path $rustRoot "Cargo.toml")
        $buildExit = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
    }
    if ($buildExit -ne 0) { throw "Rust driver build failed" }
    $fixtureTargets = switch ($suite) {
        "shared" { @("electron", "tauri", "webview") }
        "native" { @("wpf", "winui3", "webview", "electron") }
        "capture" { @("wpf", "electron") }
        default { @("wpf", "winui3", "webview", "electron", "tauri") }
    }
    & (Join-Path $scriptDir "build-harnesses.ps1") -Targets $fixtureTargets
}

if (-not (Test-Path $env:CUA_TEST_DRIVER_BIN)) {
    throw "Driver binary not found: $($env:CUA_TEST_DRIVER_BIN)"
}
$requiredFixtures = @()
$requiredFixtures += Join-Path $env:CUA_TEST_APPS_ROOT "harness-electron\CuaTestHarness.Electron.exe"
if ($suite -in @("shared", "all")) {
    $requiredFixtures += Join-Path $env:CUA_TEST_APPS_ROOT "harness-tauri\CuaTestHarness.Tauri.exe"
}
if ($suite -in @("native", "capture", "all")) {
    $requiredFixtures += Join-Path $env:CUA_TEST_APPS_ROOT "harness-wpf\CuaTestHarness.Wpf.exe"
}
if ($suite -in @("shared", "native", "all")) {
    $requiredFixtures += Join-Path $env:CUA_TEST_APPS_ROOT "harness-webview\CuaTestHarness.WebView.exe"
}
if ($suite -in @("native", "all")) {
    $requiredFixtures += @(
        (Join-Path $env:CUA_TEST_APPS_ROOT "harness-winui3\CuaTestHarness.WinUI3.exe")
    )
}
foreach ($fixture in $requiredFixtures) {
    if (-not (Test-Path $fixture)) { throw "Required fixture was not built: $fixture" }
}

function Invoke-E2eReport {
    Push-Location $rustRoot
    try {
        $previousPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $reportOutput = @(& cargo run -p cua-driver-testkit --bin cua-e2e-report -- `
                --declarations $casesPath `
                --environment $environmentPath `
                --results $resultsPath `
                --artifact-root $artifactDir `
                --require-video `
                --output $summaryPath 2>&1) | ForEach-Object {
                    if ($_ -is [System.Management.Automation.ErrorRecord]) {
                        $_.Exception.Message
                    } else {
                        $_.ToString()
                    }
                }
            $exitCode = $LASTEXITCODE
        } finally {
            $ErrorActionPreference = $previousPreference
        }
        $reportOutput | Out-Host
        return $exitCode
    } finally {
        Pop-Location
    }
}

Write-Host "[PREFLIGHT] Windows desktop, fixture, UIA, capture, and video" -ForegroundColor Yellow
Push-Location $rustRoot
try {
    $preflightLog = Join-Path $artifactDir "environment-preflight.log"
    # Windows PowerShell 5.1 wraps native stderr as ErrorRecord objects. With
    # the script-wide Stop preference, normal Cargo progress would terminate
    # the lane before LASTEXITCODE can be inspected. Relax only around the
    # captured native process; the real exit code remains authoritative.
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $preflightOutput = @(& cargo test -p cua-driver --test e2e_environment_preflight_test -- `
            --ignored --exact canonical_e2e_environment_is_ready --nocapture --test-threads=1 2>&1) | `
            ForEach-Object {
                if ($_ -is [System.Management.Automation.ErrorRecord]) {
                    $_.Exception.Message
                } else {
                    $_.ToString()
                }
            }
        $preflightExit = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
    }
    $preflightOutput | Tee-Object -FilePath $preflightLog
} finally {
    Pop-Location
}
if ($preflightExit -ne 0) {
    Invoke-E2eReport | Out-Null
    throw "Windows E2E environment preflight failed"
}

function Invoke-CargoTest {
    param([string]$Name, [string[]]$Arguments)
    Write-Host "[RUN] $Name" -ForegroundColor Yellow
    Push-Location $rustRoot
    try {
        $logPath = Join-Path $artifactDir ("$($Name -replace '[^A-Za-z0-9_.-]', '-').log")
        $previousPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $output = @(& cargo @Arguments 2>&1) | ForEach-Object {
                if ($_ -is [System.Management.Automation.ErrorRecord]) {
                    $_.Exception.Message
                } else {
                    $_.ToString()
                }
            }
            $exitCode = $LASTEXITCODE
        } finally {
            $ErrorActionPreference = $previousPreference
        }
        $output | Tee-Object -FilePath $logPath
        if ($exitCode -ne 0) {
            $script:FailureCount++
        }
    } finally {
        Pop-Location
    }
}

function Test-E2eRecordings {
    $failureCount = 0
    $errors = @(Get-ChildItem -Path $recordingRoot -Filter "recording-error.txt" -Recurse -ErrorAction SilentlyContinue)
    foreach ($errorFile in $errors) {
        Write-Host "[VIDEO FAIL] $($errorFile.FullName)" -ForegroundColor Red
        Get-Content $errorFile.FullName | Write-Host
        $failureCount++
    }

    $videos = @(Get-ChildItem -Path $recordingRoot -Filter "recording.mp4" -Recurse -ErrorAction SilentlyContinue)
    if ($videos.Count -eq 0) {
        Write-Host "[VIDEO FAIL] No E2E trajectory videos were produced" -ForegroundColor Red
        return ($failureCount + 1)
    }
    foreach ($video in $videos) {
        & $ffprobe.Source -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 $video.FullName | Out-Null
        if ($LASTEXITCODE -ne 0 -or $video.Length -eq 0) {
            Write-Host "[VIDEO FAIL] Unplayable trajectory: $($video.FullName)" -ForegroundColor Red
            $failureCount++
        } else {
            Write-Host "[VIDEO PASS] $($video.FullName)" -ForegroundColor Green
        }
    }

    $ownedVideos = @{}
    Get-Content $resultsPath | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object {
        $result = $_ | ConvertFrom-Json
        if ($null -ne $result.evidence.video) {
            $ownedVideos[$result.evidence.video.Replace("\", "/")] = $true
        }
    }
    foreach ($video in $videos) {
        $artifactPrefix = ([System.IO.Path]::GetFullPath($artifactDir)).TrimEnd("\") + "\"
        $videoPath = [System.IO.Path]::GetFullPath($video.FullName)
        if (-not $videoPath.StartsWith($artifactPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            Write-Host "[VIDEO FAIL] Trajectory escaped artifact root: $videoPath" -ForegroundColor Red
            $failureCount++
            continue
        }
        $relative = $videoPath.Substring($artifactPrefix.Length).Replace("\", "/")
        if ($relative -like "recordings/environment-preflight-*/recording.mp4") {
            continue
        }
        if (-not $ownedVideos.ContainsKey($relative)) {
            Write-Host "[VIDEO FAIL] Orphan trajectory has no typed result row: $relative" -ForegroundColor Red
            $failureCount++
        }
    }
    return $failureCount
}

$script:FailureCount = 0

if ($suite -in @("shared", "all")) {
    Invoke-CargoTest "shared behavior matrix" @(
        "test", "-p", "cua-driver", "--test", "cross_platform_behavior_test", "--",
        "--ignored", "--exact", "shared_web_action_matrix_is_state_verified",
        "--nocapture", "--test-threads=1"
    )
    Invoke-CargoTest "embedded browser routes" @(
        "test", "-p", "cua-driver", "--test", "cross_platform_behavior_test", "--",
        "--ignored", "--exact", "embedded_browser_routes_are_exact_or_refused",
        "--nocapture", "--test-threads=1"
    )
}

if ($suite -in @("native", "all")) {
    Invoke-CargoTest "Windows native harnesses" @(
        "test", "-p", "cua-driver", "--test", "harness_wpf_test", "--",
        "--ignored", "--nocapture", "--test-threads=1"
    )
    Invoke-CargoTest "WinUI3 harnesses" @(
        "test", "-p", "cua-driver", "--test", "harness_winui3_test", "--",
        "--ignored", "--nocapture", "--test-threads=1"
    )
    Invoke-CargoTest "Windows web harnesses" @(
        "test", "-p", "cua-driver", "--test", "harness_web_test", "--",
        "--ignored", "--nocapture", "--test-threads=1"
    )
    Invoke-CargoTest "Windows minimized launch" @(
        "test", "-p", "cua-driver", "--test", "launch_windows_test", "--",
        "--ignored", "--nocapture", "--test-threads=1"
    )
    Invoke-CargoTest "Windows agent cursor" @(
        "test", "-p", "cua-driver", "--test", "agent_cursor_windows_test", "--",
        "--ignored", "--nocapture", "--test-threads=1"
    )
}

if ($suite -in @("capture", "all")) {
    Invoke-CargoTest "capture contract" @(
        "test", "-p", "cua-driver", "--test", "capture_contract_test", "--",
        "--ignored", "--nocapture", "--test-threads=1"
    )
    Invoke-CargoTest "Windows desktop scope" @(
        "test", "-p", "cua-driver", "--test", "desktop_scope_windows_test", "--",
        "--ignored", "--nocapture", "--test-threads=1"
    )
}

$script:FailureCount += (Test-E2eRecordings)

$reportExit = Invoke-E2eReport
if ($reportExit -ne 0) {
    Write-Host "Windows E2E result validation failed" -ForegroundColor Red
    $script:FailureCount++
}

if ($script:FailureCount -ne 0) {
    throw "Windows Rust e2e suite had $($script:FailureCount) failing lane(s)"
}

Write-Host "Windows Rust e2e matrix completed: $suite" -ForegroundColor Green
