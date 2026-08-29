$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$required = @(
    "CUA_RESTRICTED_BROWSER_TEST_NAME",
    "CUA_RESTRICTED_BROWSER_TEST_LOG",
    "CUA_RESTRICTED_BROWSER_TEST_EXIT",
    "CUA_RESTRICTED_BROWSER_TEST_PID",
    "CUA_RESTRICTED_BROWSER_TEST_RUST_ROOT"
)
foreach ($name in $required) {
    if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name))) {
        throw "Missing required restricted-test environment variable: $name"
    }
}

$testName = $env:CUA_RESTRICTED_BROWSER_TEST_NAME
$logPath = $env:CUA_RESTRICTED_BROWSER_TEST_LOG
$exitPath = $env:CUA_RESTRICTED_BROWSER_TEST_EXIT
$pidPath = $env:CUA_RESTRICTED_BROWSER_TEST_PID
$rustRoot = $env:CUA_RESTRICTED_BROWSER_TEST_RUST_ROOT
$exitTemp = "$exitPath.tmp-$PID"
$exitCode = 1
Set-Content -LiteralPath $pidPath -Value $PID -NoNewline

try {
    Push-Location $rustRoot
    try {
        $previousPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $output = @(& cargo test --release -p cua-driver `
                --test standalone_browser_behavior_test $testName -- `
                --ignored --exact --nocapture --test-threads=1 2>&1) |
                ForEach-Object {
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
        $output | Set-Content -LiteralPath $logPath -Encoding UTF8
    } finally {
        Pop-Location
    }
} catch {
    ($_ | Out-String) | Add-Content -LiteralPath $logPath -Encoding UTF8
    $exitCode = 1
} finally {
    Set-Content -LiteralPath $exitTemp -Value $exitCode -NoNewline
    Move-Item -LiteralPath $exitTemp -Destination $exitPath
}

exit $exitCode
