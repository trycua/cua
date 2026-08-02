# sandbox-runner.ps1 - runs INSIDE Windows Sandbox as LogonCommand
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

# -- set env so binary_path() resolves cua-driver.exe correctly -------------
$repoRoot = "C:\cua-driver"
$rustRoot = Join-Path $repoRoot "rust"
$targetDeps = Join-Path $rustRoot "target\debug\deps"

$env:CARGO_MANIFEST_DIR = Join-Path $rustRoot "crates\cua-driver"
Log "CARGO_MANIFEST_DIR = $env:CARGO_MANIFEST_DIR"

$driverExe = Join-Path $rustRoot "target\debug\cua-driver.exe"
if (-not (Test-Path $driverExe)) {
    Log "ERROR: $driverExe not found"
    "1"    | Out-File $exitFile -Encoding utf8 -NoNewline
    "done" | Out-File $doneFile -Encoding utf8 -NoNewline
    exit 1
}
Log "cua-driver   : $driverExe"

# -- find test binaries -------------------------------------------------------
# Run the protocol test first, followed by the typed interactive harnesses.
$testSuites = @(
    @{ Pattern = "protocol_handshake_test-*.exe"; Label = "protocol_handshake_test" },
    @{ Pattern = "harness_wpf_test-*.exe";        Label = "harness_wpf_test";        Extra = @("--ignored") },
    @{ Pattern = "harness_winui3_test-*.exe";     Label = "harness_winui3_test";     Extra = @("--ignored") },
    @{ Pattern = "harness_web_test-*.exe";        Label = "harness_web_test";        Extra = @("--ignored") },
    @{ Pattern = "launch_windows_test-*.exe";      Label = "launch_windows_test";      Extra = @("--ignored") },
    @{ Pattern = "agent_cursor_windows_test-*.exe"; Label = "agent_cursor_windows_test"; Extra = @("--ignored") }
)

# -- stage harness binaries to %TEMP% (same Zone-3 ShellExecute workaround) --
$harnessRoots = @(
    @{ Src = (Join-Path $rustRoot "test-apps\harness-wpf");      EnvVar = "HARNESS_WPF_EXE";      Exe = "CuaTestHarness.Wpf.exe" },
    @{ Src = (Join-Path $rustRoot "test-apps\harness-winui3");   EnvVar = "HARNESS_WINUI3_EXE";   Exe = "CuaTestHarness.WinUI3.exe" },
    @{ Src = (Join-Path $rustRoot "test-apps\harness-webview");  EnvVar = "HARNESS_WEBVIEW_EXE";  Exe = "CuaTestHarness.WebView.exe" },
    @{ Src = (Join-Path $rustRoot "test-apps\harness-electron"); EnvVar = "HARNESS_ELECTRON_EXE"; Exe = "CuaTestHarness.Electron.exe" },
    @{ Src = (Join-Path $rustRoot "test-apps\harness-tauri");    EnvVar = "HARNESS_TAURI_EXE";    Exe = "CuaTestHarness.Tauri.exe" }
)
foreach ($h in $harnessRoots) {
    if (-not (Test-Path $h.Src)) {
        Log "harness $($h.Exe) not built - $($h.EnvVar) tests will skip"
        continue
    }
    $dst = Join-Path $env:TEMP (Split-Path $h.Src -Leaf)
    Copy-Item $h.Src $dst -Recurse -Force
    # Confirm the publish output actually contains the exe before exporting
    # the env var - a partial publish would otherwise turn an expected skip
    # into a hard launch failure inside the Rust test.
    $stagedExe = Join-Path $dst $h.Exe
    if (-not (Test-Path $stagedExe)) {
        Log "WARNING: staged harness exe missing at $stagedExe - $($h.EnvVar) tests will skip"
        continue
    }
    Set-Item "Env:$($h.EnvVar)" $stagedExe
    Log "$($h.Exe) staged to $dst (env $($h.EnvVar)=$stagedExe)"
}

# -- optional test filter ----------------------------------------------------
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

    $testBin = Get-ChildItem "$targetDeps\$($suite.Pattern)" |
               Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $testBin) {
        Log "WARNING: $($suite.Pattern) not found, skipping $label"
        continue
    }
    Log "Running $label : $($testBin.FullName)"

    $testArgs = @("--test-threads=1", "--nocapture")
    if ($suite.ContainsKey("Extra") -and $suite.Extra) {
        $testArgs += $suite.Extra
    }
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
