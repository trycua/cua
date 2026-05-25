# sandbox-runner.ps1 — runs INSIDE Windows Sandbox as LogonCommand
# Streams test output live to C:\sandbox-output\ so the host can tail it.

$ErrorActionPreference = "Continue"
$logFile    = "C:\sandbox-output\runner-log.txt"
$outputFile = "C:\sandbox-output\test-output.txt"
$exitFile   = "C:\sandbox-output\exit-code.txt"
$doneFile   = "C:\sandbox-output\done.txt"

function Log($msg) {
    $ts = (Get-Date).ToString("HH:mm:ss")
    "$ts  $msg" | Tee-Object -FilePath $logFile -Append | Write-Host
}

Log "sandbox-runner.ps1 starting"

# ── set env so binary_path() resolves cua-driver.exe correctly ─────────────
$env:CARGO_MANIFEST_DIR = "C:\cua-driver-rs\crates\cua-driver"
Log "CARGO_MANIFEST_DIR = $env:CARGO_MANIFEST_DIR"

# ── copy test-app exe to %TEMP% so ShellExecuteW doesn't hit a zone dialog ──
# Files on Sandbox-mapped folders are Zone 3 (Internet); ShellExecuteW blocks
# on the "Do you want to run this file?" dialog.  A local %TEMP% copy is Zone 1.
$testAppSrc = "C:\cua-driver-rs\test-apps\desktop-test-app-electron.0.1.0.exe"
$testAppDst = "$env:TEMP\desktop-test-app-electron.0.1.0.exe"
if (Test-Path $testAppSrc) {
    Copy-Item $testAppSrc $testAppDst -Force
    $env:TEST_APP_EXE = $testAppDst
    Log "test-app copied to $testAppDst (TEST_APP_EXE=$testAppDst)"
} else {
    Log "WARNING: test-app not found at $testAppSrc"
}

$driverExe = "C:\cua-driver-rs\target\debug\cua-driver.exe"
if (-not (Test-Path $driverExe)) {
    Log "ERROR: $driverExe not found"
    "1"    | Out-File $exitFile -Encoding utf8 -NoNewline
    "done" | Out-File $doneFile -Encoding utf8 -NoNewline
    exit 1
}
Log "cua-driver   : $driverExe"

# ── find test binaries ───────────────────────────────────────────────────────
# Run mcp_protocol_test first, then ux_guard_test (UX guard needs a real
# desktop session and spawns visible windows, so it runs second).
$testSuites = @(
    @{ Pattern = "mcp_protocol_test-*.exe"; Label = "mcp_protocol_test" },
    @{ Pattern = "ux_guard_test-*.exe";      Label = "ux_guard_test" }
)

# ── optional test filter ────────────────────────────────────────────────────
$filterSuite = ""
if (Test-Path "C:\sandbox-output\test-filter.txt") {
    $filterSuite = (Get-Content "C:\sandbox-output\test-filter.txt" -Raw).Trim()
}
$filterLabel = if ($filterSuite) { $filterSuite } else { "(all)" }
Log "Suite filter : $filterLabel"

"" | Out-File $outputFile -Encoding utf8  # create file so host can start tailing

$overallExit = 0

foreach ($suite in $testSuites) {
    $label = $suite.Label
    # If a filter was given and this suite's label doesn't match, skip it.
    if ($filterSuite -ne "" -and $label -notlike "*$filterSuite*") {
        Log "Skipping $label (filter='$filterSuite')"
        continue
    }

    $testBin = Get-ChildItem "C:\cua-driver-rs\target\debug\deps\$($suite.Pattern)" |
               Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $testBin) {
        Log "WARNING: $($suite.Pattern) not found, skipping $label"
        continue
    }
    Log "Running $label : $($testBin.FullName)"

    $testArgs = @("--test-threads=1", "--nocapture")
    $stderrFile = "C:\sandbox-output\$label-stderr.txt"

    "`n=== $label ===`n" | Add-Content $outputFile -Encoding utf8

    $proc = Start-Process `
        -FilePath       $testBin.FullName `
        -ArgumentList   $testArgs `
        -RedirectStandardOutput "C:\sandbox-output\$label-stdout.txt" `
        -RedirectStandardError  $stderrFile `
        -NoNewWindow `
        -PassThru `
        -Wait

    # Append stdout to the shared output file.
    if (Test-Path "C:\sandbox-output\$label-stdout.txt") {
        Get-Content "C:\sandbox-output\$label-stdout.txt" -Raw |
            Add-Content $outputFile -Encoding utf8
    }
    # Append stderr.
    if (Test-Path $stderrFile) {
        $stderr = Get-Content $stderrFile -Raw
        if ($stderr.Trim() -ne "") {
            "`n--- $label stderr ---`n$stderr" | Add-Content $outputFile -Encoding utf8
        }
    }

    $suiteExit = $proc.ExitCode
    Log "$label exited : $suiteExit"
    if ($suiteExit -ne 0) { $overallExit = $suiteExit }
}

$overallExit.ToString() | Out-File $exitFile -Encoding utf8 -NoNewline
Log "Results written, writing done sentinel..."
"done" | Out-File $doneFile -Encoding utf8 -NoNewline
Log "Finished."
