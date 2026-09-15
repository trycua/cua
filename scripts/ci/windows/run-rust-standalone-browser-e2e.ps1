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
$env:CUA_E2E_BROWSER_PROVENANCE_FILE = Join-Path $artifactDir "browser-provenance.jsonl"
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
    $env:CUA_E2E_BROWSER_PROVENANCE_FILE,
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

function Test-IsAdministrator {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [System.Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Invoke-RestrictedBrowserTest {
    param(
        [Parameter(Mandatory = $true)][string]$TestName,
        [Parameter(Mandatory = $true)][string]$LogPath
    )

    $sourceHelper = Join-Path $PSScriptRoot "run-restricted-browser-test.ps1"
    if (-not (Test-Path -LiteralPath $sourceHelper)) {
        throw "Restricted browser-test helper not found: $sourceHelper"
    }

    $testExecutables = @(Get-ChildItem (Join-Path $rustRoot "target\release\deps") `
        -Filter "standalone_browser_behavior_test-*.exe" -File)
    if ($testExecutables.Count -ne 1) {
        throw "Expected one compiled standalone-browser test executable, found $($testExecutables.Count)"
    }

    # A SAFER/restricted token keeps the runner user's primary SID but makes
    # the Administrators group deny-only. GitHub's D:\a workspace is writable
    # through that group, so the child could launch and then fail before it
    # could write its pid/exit files. Stage the executable and all child-owned
    # state under the user's LocalAppData instead. The parent copies only the
    # finished evidence back into the workflow artifact directory.
    $restrictedRoot = Join-Path $env:LOCALAPPDATA `
        ("CuaDriverE2E\restricted-{0}-{1}" -f $TestName, [guid]::NewGuid().ToString("N"))
    $restrictedEvidence = Join-Path $restrictedRoot "evidence"
    $restrictedJournals = Join-Path $restrictedRoot "journals"
    $restrictedRecordings = Join-Path $restrictedEvidence "recordings"
    New-Item -ItemType Directory -Force -Path @(
        $restrictedEvidence,
        $restrictedJournals,
        $restrictedRecordings
    ) | Out-Null

    $helper = Join-Path $restrictedRoot "run-restricted-browser-test.ps1"
    $testExecutable = Join-Path $restrictedRoot "standalone_browser_behavior_test.exe"
    $stagedDriver = Join-Path $restrictedRoot "cua-driver.exe"
    $childLogPath = Join-Path $restrictedRoot "test.log"
    $exitPath = Join-Path $restrictedRoot "test.exit"
    $pidPath = Join-Path $restrictedRoot "test.pid"
    $configPath = Join-Path $restrictedRoot "test.config.json"
    Copy-Item -LiteralPath $sourceHelper -Destination $helper
    Copy-Item -LiteralPath $testExecutables[0].FullName -Destination $testExecutable
    Copy-Item -LiteralPath $env:CUA_TEST_DRIVER_BIN -Destination $stagedDriver

    $childEnvironment = [ordered]@{}
    foreach ($name in @(
        "PATH",
        "RUST_BACKTRACE",
        "CUA_E2E_SOURCE_SHA",
        "CUA_E2E_FORBID_SKIPS",
        "CUA_E2E_BROWSER_STDERR",
        "CUA_TEST_APPS_ROOT",
        "CUA_TEST_DRIVER_STDERR",
        "CUA_TEST_REQUIRE_EXTERNAL_BROWSERS",
        "CUA_TEST_REQUIRE_PROTECTED_WINDOWS_BROWSER",
        "CUA_TEST_WORKSPACE_ROOT"
    )) {
        $value = [Environment]::GetEnvironmentVariable($name, "Process")
        if (-not [string]::IsNullOrWhiteSpace($value)) {
            $childEnvironment[$name] = $value
        }
    }
    $childEnvironment["CUA_E2E_WINDOWS_BROWSER_LIMITATION"] = "0"
    $childEnvironment["CUA_TEST_DRIVER_BIN"] = $stagedDriver
    $childEnvironment["CUA_E2E_ARTIFACT_DIR"] = $restrictedEvidence
    $childEnvironment["CUA_E2E_DECLARATIONS_FILE"] = Join-Path $restrictedJournals "cases.jsonl"
    $childEnvironment["CUA_E2E_ENVIRONMENT_FILE"] = Join-Path $restrictedJournals "environment.jsonl"
    $childEnvironment["CUA_E2E_BROWSER_PROVENANCE_FILE"] = Join-Path $restrictedJournals "browser-provenance.jsonl"
    $childEnvironment["CUA_E2E_RESULTS_FILE"] = Join-Path $restrictedJournals "results.jsonl"
    $childEnvironment["CUA_E2E_RECORDINGS_ROOT"] = $restrictedRecordings
    foreach ($path in @(
        $childEnvironment["CUA_E2E_DECLARATIONS_FILE"],
        $childEnvironment["CUA_E2E_ENVIRONMENT_FILE"],
        $childEnvironment["CUA_E2E_BROWSER_PROVENANCE_FILE"],
        $childEnvironment["CUA_E2E_RESULTS_FILE"]
    )) {
        New-Item -ItemType File -Path $path | Out-Null
    }
    if (-not [string]::IsNullOrWhiteSpace($env:CUA_E2E_SOURCE_MARKER) -and
        (Test-Path -LiteralPath $env:CUA_E2E_SOURCE_MARKER)) {
        $stagedSourceMarker = Join-Path $restrictedRoot ".cua-e2e-source-sha"
        Copy-Item -LiteralPath $env:CUA_E2E_SOURCE_MARKER -Destination $stagedSourceMarker
        $childEnvironment["CUA_E2E_SOURCE_MARKER"] = $stagedSourceMarker
    }
    [ordered]@{
        test_name = $TestName
        log_path = $childLogPath
        exit_path = $exitPath
        pid_path = $pidPath
        test_executable = $testExecutable
        environment = $childEnvironment
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $configPath -Encoding UTF8
    New-Item -ItemType File -Path $LogPath | Out-Null
    "Launching prebuilt test executable with Windows trust level 0x20000" |
        Add-Content -LiteralPath $LogPath -Encoding UTF8

    try {
        $command = "powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$helper`" -ConfigPath `"$configPath`""
        $previousPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $runasOutput = @(& "$env:SystemRoot\System32\runas.exe" /trustlevel:0x20000 $command 2>&1) |
                ForEach-Object {
                    if ($_ -is [System.Management.Automation.ErrorRecord]) {
                        $_.Exception.Message
                    } else {
                        $_.ToString()
                    }
                }
            $runasExit = $LASTEXITCODE
        } finally {
            $ErrorActionPreference = $previousPreference
        }
        $runasOutput | Add-Content -LiteralPath $LogPath -Encoding UTF8
        if ($runasExit -ne 0) {
            "runas exited before launching the restricted test: $runasExit" |
                Add-Content -LiteralPath $LogPath -Encoding UTF8
            Get-Content -LiteralPath $LogPath | Out-Host
            return $runasExit
        }

        $deadline = (Get-Date).AddMinutes(3)
        while (-not (Test-Path -LiteralPath $exitPath)) {
            if ((Get-Date) -ge $deadline) {
                if (Test-Path -LiteralPath $pidPath) {
                    $childPid = [int](Get-Content -LiteralPath $pidPath -Raw)
                    & "$env:SystemRoot\System32\taskkill.exe" /PID $childPid /T /F | Out-Host
                }
                "Restricted browser test timed out after 3 minutes" |
                    Add-Content -LiteralPath $LogPath -Encoding UTF8
                return 124
            }
            Start-Sleep -Milliseconds 250
        }
        return [int](Get-Content -LiteralPath $exitPath -Raw)
    } finally {
        if (Test-Path -LiteralPath $childLogPath) {
            Get-Content -LiteralPath $childLogPath |
                Add-Content -LiteralPath $LogPath -Encoding UTF8
        }
        $journalDestinations = [ordered]@{
            "cases.jsonl" = $env:CUA_E2E_DECLARATIONS_FILE
            "environment.jsonl" = $env:CUA_E2E_ENVIRONMENT_FILE
            "browser-provenance.jsonl" = $env:CUA_E2E_BROWSER_PROVENANCE_FILE
            "results.jsonl" = $env:CUA_E2E_RESULTS_FILE
        }
        foreach ($entry in $journalDestinations.GetEnumerator()) {
            $source = Join-Path $restrictedJournals $entry.Key
            if ((Test-Path -LiteralPath $source) -and (Get-Item $source).Length -gt 0) {
                Get-Content -LiteralPath $source |
                    Add-Content -LiteralPath $entry.Value -Encoding UTF8
            }
        }
        foreach ($item in @(Get-ChildItem -Force $restrictedEvidence -ErrorAction SilentlyContinue)) {
            Copy-Item -LiteralPath $item.FullName -Destination $artifactDir -Recurse -Force
        }
        Get-Content -LiteralPath $LogPath | Out-Host
        Remove-Item -LiteralPath $restrictedRoot -Recurse -Force -ErrorAction SilentlyContinue
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

$jobLifecycleExit = Invoke-CargoStep -Name "Windows browser job lifecycle" -Arguments @(
    "test", "--release", "-p", "cua-driver-core",
    "windows_job_reaps_descendant_after_clean_launcher_handoff", "--lib", "--",
    "--nocapture"
) -LogPath (Join-Path $artifactDir "windows-browser-job-lifecycle.log")
if ($jobLifecycleExit -ne 0) {
    throw "Windows browser Job Object lifecycle test failed with exit code $jobLifecycleExit"
}

$tests = @()
$runRestricted = Test-IsAdministrator
if ($runRestricted -and
    $env:CUA_E2E_WINDOWS_BROWSER_LIMITATION -eq "hosted_runner_token") {
    # Preserve the hosted administrator-token negative control before using a
    # restricted child for the positive platform-selected rows.
    $tests += "standalone_browser_prepare_isolated_hosted_token_refusal"
}
$tests += @(
    "standalone_browser_prepare_automation_exposure",
    "standalone_browser_prepare_isolated",
    "standalone_browser_background_type",
    "standalone_browser_type_replace",
    "standalone_browser_owned_permission_prompt",
    "standalone_browser_dialogs",
    "standalone_browser_download",
    "standalone_browser_existing_profile",
    "standalone_browser_existing_profile_setup",
    "standalone_browser_frames",
    "standalone_browser_multi_tab",
    "standalone_browser_pointer_actions",
    "standalone_browser_roundtrip",
    "standalone_browser_semantic_state",
    "standalone_browser_stale_ref",
    "standalone_browser_trust_gated_dom_click",
    "standalone_browser_trusted_click",
    "standalone_browser_upload",
    "standalone_browser_window_collision"
)
$restrictedBrowserTests = @(
    "standalone_browser_prepare_automation_exposure",
    "standalone_browser_prepare_isolated"
)
if ($runRestricted) {
    Write-Host "Administrator token detected; platform-selected browser tests will use Windows trust level 0x20000"
    Get-ChildItem (Join-Path $rustRoot "target\release\deps") `
        -Filter "standalone_browser_behavior_test-*" -File -ErrorAction SilentlyContinue |
        Remove-Item -Force
    $testBuildExit = Invoke-CargoStep -Name "standalone browser test executable" -Arguments @(
        "test", "--release", "-p", "cua-driver",
        "--test", "standalone_browser_behavior_test", "--no-run"
    ) -LogPath (Join-Path $artifactDir "build-standalone-browser-test.log")
    if ($testBuildExit -ne 0) {
        throw "standalone browser test executable build failed with exit code $testBuildExit"
    }
}
$failureCount = 0
foreach ($testName in $tests) {
    $logPath = Join-Path $artifactDir "$testName.log"
    if ($runRestricted -and $restrictedBrowserTests -contains $testName) {
        $testExit = Invoke-RestrictedBrowserTest -TestName $testName -LogPath $logPath
    } else {
        $testExit = Invoke-CargoStep -Name $testName -Arguments @(
            "test", "--release", "-p", "cua-driver",
            "--test", "standalone_browser_behavior_test", $testName, "--",
            "--ignored", "--exact", "--nocapture", "--test-threads=1"
        ) -LogPath $logPath
    }
    if ($testExit -ne 0) {
        $failureCount++
    }
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
