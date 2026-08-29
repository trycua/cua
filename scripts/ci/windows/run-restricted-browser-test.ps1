param(
    [Parameter(Mandatory = $true)][string]$ConfigPath
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$config = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
foreach ($property in $config.environment.PSObject.Properties) {
    [Environment]::SetEnvironmentVariable($property.Name, [string]$property.Value, "Process")
}

$testName = [string]$config.test_name
$logPath = [string]$config.log_path
$exitPath = [string]$config.exit_path
$pidPath = [string]$config.pid_path
$rustRoot = [string]$config.rust_root
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
        $output | Add-Content -LiteralPath $logPath -Encoding UTF8
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
