# run-tests-in-sandbox.ps1  — host-side launcher with live log streaming
#
# Usage (from workspace root):
#   .\sandbox\run-tests-in-sandbox.ps1 [<test-filter>]
#
# Steps:
#   1. cargo build + cargo test --no-run
#   2. Launch Windows Sandbox with workspace mapped read-only
#   3. Tail runner-log.txt and test-output.txt live (2s poll)
#   4. Exit with sandbox test exit code

param([string]$TestFilter = "")

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Close-Sandbox {
    Write-Host "`n[SANDBOX] Closing sandbox..." -ForegroundColor Yellow
    $names = @("WindowsSandboxClient", "WindowsSandbox")
    foreach ($n in $names) {
        $procs = Get-Process $n -ErrorAction SilentlyContinue
        if ($procs) {
            Write-Host "  Stopping $n (pid $($procs.Id -join ','))..."
            $procs | Stop-Process -Force -ErrorAction SilentlyContinue
        }
    }
    Start-Sleep -Milliseconds 1500
    $ErrorActionPreference = "Continue"
    foreach ($n in $names) { & taskkill /F /IM "$n.exe" }
    $ErrorActionPreference = "Stop"
    Write-Host "  Sandbox closed." -ForegroundColor Green
}

$sandboxDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$wsRoot     = Split-Path -Parent $sandboxDir

Write-Host "=== cua-driver-rs Windows Sandbox Test Runner ===" -ForegroundColor Cyan
Write-Host "Workspace: $wsRoot"

# ── Pre-flight: fail fast if a sandbox is already running ────────────────────
$running = Get-Process "WindowsSandboxClient","WindowsSandbox" -ErrorAction SilentlyContinue
if ($running) {
    Write-Host "`n[ERROR] Windows Sandbox is already running (pid $($running.Id -join ','))." -ForegroundColor Red
    Write-Host "        Only one sandbox instance is allowed at a time." -ForegroundColor Red
    Write-Host "        Close it first, or run:  Close-Sandbox" -ForegroundColor Yellow
    Write-Host "        Killing existing sandbox and retrying..." -ForegroundColor Yellow
    Close-Sandbox
    $running = Get-Process "WindowsSandboxClient","WindowsSandbox" -ErrorAction SilentlyContinue
    if ($running) {
        Write-Host "[ERROR] Could not kill existing sandbox. Aborting." -ForegroundColor Red
        exit 1
    }
    Write-Host "[OK] Existing sandbox closed. Proceeding." -ForegroundColor Green
}

# ── 0. Download test-app binaries if missing ─────────────────────────────────
$testAppExe = "$wsRoot\test-apps\desktop-test-app-electron.0.1.0.exe"
if (-not (Test-Path $testAppExe)) {
    Write-Host "`n[FETCH] Downloading desktop-test-app-electron..." -ForegroundColor Yellow
    New-Item -ItemType Directory -Force "$wsRoot\test-apps" | Out-Null
    $url = "https://github.com/trycua/desktop-test-app-electron/releases/download/v0.1.0/desktop-test-app-electron.0.1.0.exe"
    Invoke-WebRequest -Uri $url -OutFile $testAppExe -UseBasicParsing
    Write-Host "  Downloaded: $testAppExe" -ForegroundColor Green
} else {
    Write-Host "`n[FETCH] test-app already present: $testAppExe" -ForegroundColor Green
}

# ── 1. Build ──────────────────────────────────────────────────────────────────
Push-Location $wsRoot
try {
    Write-Host "`n[BUILD] cargo build..." -ForegroundColor Yellow
    cargo build
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    Write-Host "`n[BUILD] cargo test --no-run (mcp_protocol_test)..." -ForegroundColor Yellow
    cargo test --test mcp_protocol_test --no-run
    if ($LASTEXITCODE -ne 0) { throw "cargo test --no-run (mcp_protocol_test) failed" }

    Write-Host "`n[BUILD] cargo test --no-run (ux_guard_test)..." -ForegroundColor Yellow
    cargo test --test ux_guard_test --no-run
    if ($LASTEXITCODE -ne 0) { throw "cargo test --no-run (ux_guard_test) failed" }
} finally { Pop-Location }

$mcpBin = Get-ChildItem "$wsRoot\target\debug\deps\mcp_protocol_test-*.exe" |
          Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $mcpBin) { throw "mcp_protocol_test-*.exe not found" }
Write-Host "mcp_protocol_test: $($mcpBin.Name)"

$uxBin = Get-ChildItem "$wsRoot\target\debug\deps\ux_guard_test-*.exe" |
         Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($uxBin) { Write-Host "ux_guard_test    : $($uxBin.Name)" } else { Write-Host "ux_guard_test    : (not found, will skip)" }

# ── 2. Prepare shared output folder ──────────────────────────────────────────
$outputDir = "$env:TEMP\cua-sandbox-output"
New-Item -ItemType Directory -Force $outputDir | Out-Null
Get-ChildItem $outputDir -ErrorAction SilentlyContinue | Remove-Item -Force -Recurse -ErrorAction SilentlyContinue
$TestFilter | Out-File "$outputDir\test-filter.txt" -Encoding utf8 -NoNewline

# ── 3. Generate .wsb and launch ───────────────────────────────────────────────
$wsbContent = @"
<Configuration>
  <MappedFolders>
    <MappedFolder>
      <HostFolder>$wsRoot</HostFolder>
      <SandboxFolder>C:\cua-driver-rs</SandboxFolder>
      <ReadOnly>true</ReadOnly>
    </MappedFolder>
    <MappedFolder>
      <HostFolder>$outputDir</HostFolder>
      <SandboxFolder>C:\sandbox-output</SandboxFolder>
      <ReadOnly>false</ReadOnly>
    </MappedFolder>
  </MappedFolders>
  <LogonCommand>
    <Command>powershell -ExecutionPolicy Bypass -NonInteractive -File C:\cua-driver-rs\sandbox\sandbox-runner.ps1</Command>
  </LogonCommand>
</Configuration>
"@
$wsbPath = "$env:TEMP\cua-driver-test.wsb"
$wsbContent | Out-File $wsbPath -Encoding utf8

Write-Host "`n[SANDBOX] Launching Windows Sandbox..." -ForegroundColor Yellow
$preLaunchCheck = Get-Process "WindowsSandboxClient","WindowsSandbox" -ErrorAction SilentlyContinue
if ($preLaunchCheck) {
    Write-Host "[ERROR] A sandbox appeared just before launch. Aborting." -ForegroundColor Red
    exit 1
}
Start-Process $wsbPath

# ── 4. Stream logs live ────────────────────────────────────────────────────────
Write-Host "[STREAM] Tailing logs (Ctrl-C to abort)..." -ForegroundColor Yellow

$doneFile     = "$outputDir\done.txt"
$logFile      = "$outputDir\runner-log.txt"
$outputFile   = "$outputDir\test-output.txt"
$logPos       = 0
$outPos       = 0
$timeout      = 600
$elapsed      = 0
$pollInterval = 2

while ($elapsed -lt $timeout) {
    if (Test-Path $logFile) {
        $content = Get-Content $logFile -Raw -Encoding utf8 -ErrorAction SilentlyContinue
        if ($content -and $content.Length -gt $logPos) {
            $newText = $content.Substring($logPos)
            $logPos  = $content.Length
            $newText.TrimEnd() -split "`n" | ForEach-Object { Write-Host "[sandbox] $_" -ForegroundColor DarkCyan }
        }
    }
    if (Test-Path $outputFile) {
        $content = Get-Content $outputFile -Raw -Encoding utf8 -ErrorAction SilentlyContinue
        if ($content -and $content.Length -gt $outPos) {
            $newText = $content.Substring($outPos)
            $outPos  = $content.Length
            $newText.TrimEnd() -split "`n" | ForEach-Object { Write-Host "  $_" }
        }
    }
    if (Test-Path $doneFile) { break }
    Start-Sleep -Seconds $pollInterval
    $elapsed += $pollInterval
}

# Final drain
foreach ($f in @($logFile, $outputFile)) {
    if (Test-Path $f) {
        $content = Get-Content $f -Raw -Encoding utf8 -ErrorAction SilentlyContinue
        $pos = if ($f -eq $logFile) { $logPos } else { $outPos }
        if ($content -and $content.Length -gt $pos) {
            $content.Substring($pos).TrimEnd() -split "`n" | ForEach-Object { Write-Host "  $_" }
        }
    }
}

if (-not (Test-Path $doneFile)) {
    Write-Host "`nTIMEOUT after ${elapsed}s." -ForegroundColor Red
    Close-Sandbox
    exit 1
}

$exitCode = 0
if (Test-Path "$outputDir\exit-code.txt") {
    $exitCode = [int](Get-Content "$outputDir\exit-code.txt" -Raw).Trim()
}

Write-Host ""
if ($exitCode -eq 0) { Write-Host "PASSED (exit 0)" -ForegroundColor Green } else { Write-Host "FAILED (exit $exitCode)" -ForegroundColor Red }

Close-Sandbox
exit $exitCode
