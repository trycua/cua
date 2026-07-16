# Run installed Chrome/Edge browser scenarios in independent Windows processes.
param(
    [switch]$NoBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$repoRoot = (Resolve-Path (Join-Path $scriptDir "..\..\..")).Path
$rustRoot = Join-Path $repoRoot "libs\cua-driver\rust"
$artifactDir = if ([string]::IsNullOrWhiteSpace($env:CUA_E2E_ARTIFACT_DIR)) {
    Join-Path $repoRoot "artifacts\cua-driver\standalone-browser"
} else {
    [System.IO.Path]::GetFullPath($env:CUA_E2E_ARTIFACT_DIR)
}

if ((Test-Path $artifactDir) -and
    $null -ne (Get-ChildItem -Force $artifactDir -ErrorAction SilentlyContinue | Select-Object -First 1)) {
    throw "Standalone-browser artifact directory is not empty: $artifactDir. Use a fresh CUA_E2E_ARTIFACT_DIR."
}

$recordings = Join-Path $artifactDir "recordings"
New-Item -ItemType Directory -Force $recordings | Out-Null
$env:CUA_E2E_DECLARATIONS_FILE = Join-Path $artifactDir "cases.jsonl"
$env:CUA_E2E_ENVIRONMENT_FILE = Join-Path $artifactDir "environment.jsonl"
$env:CUA_E2E_RESULTS_FILE = Join-Path $artifactDir "results.jsonl"
$env:CUA_E2E_RECORDINGS_ROOT = $recordings
$env:CUA_TEST_WORKSPACE_ROOT = $rustRoot
$env:CUA_TEST_DRIVER_BIN = Join-Path $rustRoot "target\release\cua-driver.exe"
$env:CUA_TEST_APPS_ROOT = Join-Path $rustRoot "test-apps"
$env:CUA_TEST_REQUIRE_EXTERNAL_BROWSERS = "1"
$env:CUA_E2E_FORBID_SKIPS = "1"
$env:CUA_TEST_DRIVER_STDERR = "1"

foreach ($path in @(
    $env:CUA_E2E_DECLARATIONS_FILE,
    $env:CUA_E2E_ENVIRONMENT_FILE,
    $env:CUA_E2E_RESULTS_FILE
)) {
    New-Item -ItemType File -Path $path | Out-Null
}

if ([string]::IsNullOrWhiteSpace($env:CUA_E2E_SOURCE_SHA)) {
    $sourceMarker = Join-Path $repoRoot ".cua-e2e-source-sha"
    if (Test-Path $sourceMarker) {
        $env:CUA_E2E_SOURCE_MARKER = $sourceMarker
        $env:CUA_E2E_SOURCE_SHA = (Get-Content -Raw $sourceMarker).Trim()
    } else {
        $env:CUA_E2E_SOURCE_SHA = (& git -C $repoRoot rev-parse HEAD).Trim()
    }
}
if ($env:CUA_E2E_SOURCE_SHA -notmatch "^[0-9a-fA-F]{40}$") {
    throw "CUA_E2E_SOURCE_SHA must be a full 40-character commit SHA"
}

foreach ($tool in @("ffmpeg.exe", "ffprobe.exe")) {
    if ($null -eq (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "$tool is required for standalone-browser trajectory evidence"
    }
}

& (Join-Path $scriptDir "verify-user-session.ps1")

$sentinelFixture = Join-Path $rustRoot "test-apps\harness-electron\CuaTestHarness.Electron.exe"
if (-not (Test-Path $sentinelFixture)) {
    Write-Host "[FIXTURE] Staging the Electron foreground sentinel" -ForegroundColor Yellow
    & (Join-Path $scriptDir "build-harnesses.ps1") -Targets @("electron")
}
if (-not (Test-Path $sentinelFixture)) {
    throw "Electron foreground sentinel was not staged: $sentinelFixture"
}

function Invoke-CargoStep {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments,
        [Parameter(Mandatory = $true)]
        [string]$LogPath
    )

    Write-Host "[RUN] $Name" -ForegroundColor Yellow
    Push-Location $rustRoot
    try {
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
        $output | Tee-Object -FilePath $LogPath | Out-Host
        return $exitCode
    } finally {
        Pop-Location
    }
}

if (-not $NoBuild) {
    $buildExit = Invoke-CargoStep -Name "source driver" -Arguments @(
        "build", "--release", "-p", "cua-driver"
    ) -LogPath (Join-Path $artifactDir "build.log")
    if ($buildExit -ne 0) { throw "cua-driver build failed with exit code $buildExit" }
}
if (-not (Test-Path $env:CUA_TEST_DRIVER_BIN)) {
    throw "Driver binary not found: $($env:CUA_TEST_DRIVER_BIN)"
}

$tests = @(
    "standalone_browser_background_type",
    "standalone_browser_frames",
    "standalone_browser_multi_tab",
    "standalone_browser_prepare_isolated",
    "standalone_browser_roundtrip",
    "standalone_browser_stale_ref",
    "standalone_browser_trusted_click",
    "standalone_browser_window_collision"
)
$failureCount = 0
foreach ($testName in $tests) {
    $testExit = Invoke-CargoStep -Name $testName -Arguments @(
        "test", "--release", "-p", "cua-driver",
        "--test", "standalone_browser_behavior_test", $testName, "--",
        "--ignored", "--exact", "--nocapture", "--test-threads=1"
    ) -LogPath (Join-Path $artifactDir "$testName.log")
    if ($testExit -ne 0) { $failureCount++ }
}

$reportExit = Invoke-CargoStep -Name "standalone browser report" -Arguments @(
    "run", "--release", "-p", "cua-driver-testkit", "--bin", "cua-e2e-report", "--",
    "--declarations", $env:CUA_E2E_DECLARATIONS_FILE,
    "--environment", $env:CUA_E2E_ENVIRONMENT_FILE,
    "--results", $env:CUA_E2E_RESULTS_FILE,
    "--artifact-root", $artifactDir,
    "--require-video",
    "--output", (Join-Path $artifactDir "summary.md")
) -LogPath (Join-Path $artifactDir "report.log")
if ($reportExit -ne 0) { $failureCount++ }

if ($failureCount -ne 0) {
    throw "Standalone-browser E2E had $failureCount failing step(s)"
}
Write-Host "Standalone-browser E2E completed" -ForegroundColor Green
