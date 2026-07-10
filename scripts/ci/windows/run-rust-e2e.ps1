# Run the canonical Rust desktop matrix in an active Windows user session.
# Scenario definitions and assertions stay in the Rust integration test.
param(
    [switch]$NoBuild,
    [ValidateSet("default", "guard", "shared", "native", "modality", "all")]
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
$resultsPath = Join-Path $artifactDir "results.jsonl"
$summaryPath = Join-Path $artifactDir "summary.md"
@(
    "# CUA Rust Windows E2E matrix",
    "",
    "| Platform | Host/lane | Scenario | Status | Duration | Details |",
    "| --- | --- | --- | --- | --- | --- |"
) | Set-Content -Path $summaryPath
Remove-Item -Force -ErrorAction SilentlyContinue $resultsPath
$env:CUA_E2E_RESULTS_FILE = $resultsPath
$env:CUA_E2E_SUMMARY_FILE = $summaryPath

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

if ($Suite -in @("default", "guard", "modality", "all")) {
    & cargo build -p focus-monitor-win --manifest-path (Join-Path $rustRoot "Cargo.toml")
    if ($LASTEXITCODE -ne 0) { throw "Focus monitor build failed" }
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
        $logPath = Join-Path $artifactDir ("$($Name -replace '[^A-Za-z0-9_.-]', '-').log")
        $output = & cargo @Arguments 2>&1
        $exitCode = $LASTEXITCODE
        $output | Tee-Object -FilePath $logPath
        foreach ($line in $output) {
            $match = [regex]::Match(
                [string]$line,
                '^\s*test\s+(?<name>\S+)\s+\.\.\.\s+(?<status>ok|FAILED|ignored)\s*$'
            )
            if (-not $match.Success) { continue }

            $testStatus = switch ($match.Groups["status"].Value) {
                "ok" { "PASS" }
                "FAILED" { "FAIL" }
                default { "SKIP" }
            }
            $testName = $match.Groups["name"].Value
            $testMessage = if ($testStatus -eq "FAIL") { "test case failed; see lane log" } else { "" }
            $testRecord = [ordered]@{
                schema = "cua-e2e-result/v1"
                platform = "windows"
                host = "cargo"
                scenario = $testName
                status = $testStatus
                message = $testMessage
            } | ConvertTo-Json -Compress
            Add-Content -Path $resultsPath -Value $testRecord
            $testDetails = if ($testMessage) { $testMessage } else { "-" }
            Add-Content -Path $summaryPath -Value "| Windows | cargo | $testName | $testStatus | n/a | $testDetails |"
        }
        $status = if ($exitCode -eq 0) { "PASS" } else { "FAIL" }
        $record = [ordered]@{
            schema = "cua-e2e-result/v1"
            platform = "windows"
            host = "lane"
            scenario = $Name
            status = $status
            message = if ($exitCode -eq 0) { "" } else { "exit code $exitCode" }
        } | ConvertTo-Json -Compress
        Add-Content -Path $resultsPath -Value $record
        $details = if ($exitCode -eq 0) { "-" } else { "exit code $exitCode" }
        Add-Content -Path $summaryPath -Value "| Windows | lane | $Name | $status | n/a | $details |"
        if ($exitCode -ne 0) {
            $script:FailureCount++
        }
    } finally {
        Pop-Location
    }
}

$script:FailureCount = 0

if ($Suite -in @("shared", "all")) {
    Invoke-CargoTest "shared behavior matrix" @(
        "test", "-p", "cua-driver", "--test", "cross_platform_behavior_test", "--",
        "--ignored", "--nocapture", "--test-threads=1"
    )
}

if ($Suite -in @("default", "all")) {
    Invoke-CargoTest "default Rust tests" @(
        "test", "-p", "cua-driver", "-p", "platform-windows", "--",
        "--nocapture", "--test-threads=1"
    )
}

if ($Suite -in @("guard", "all")) {
    Invoke-CargoTest "guard UX" @(
        "test", "-p", "cua-driver", "--test", "guard_ux_test", "--",
        "--nocapture", "--test-threads=1"
    )
}

if ($Suite -in @("native", "all")) {
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
}

if ($Suite -in @("modality", "all")) {
    Invoke-CargoTest "Windows modality input e2e" @(
        "test", "-p", "cua-driver", "--test", "modality_input_e2e_test", "--",
        "--ignored", "--nocapture", "--test-threads=1"
    )
}

if ($script:FailureCount -ne 0) {
    throw "Windows Rust e2e suite had $($script:FailureCount) failing lane(s)"
}

Write-Host "Windows Rust e2e suite completed: $Suite" -ForegroundColor Green
