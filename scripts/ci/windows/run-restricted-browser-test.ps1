param(
    [Parameter(Mandatory = $true)][string]$ConfigPath
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$config = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
foreach ($item in @(Get-ChildItem Env:)) {
    if ($item.Name.StartsWith("CUA_") -or
        $item.Name.StartsWith("RUST") -or
        $item.Name.StartsWith("CARGO_")) {
        [Environment]::SetEnvironmentVariable($item.Name, $null, "Process")
    }
}
foreach ($property in $config.environment.PSObject.Properties) {
    [Environment]::SetEnvironmentVariable($property.Name, [string]$property.Value, "Process")
}

$testName = [string]$config.test_name
$logPath = [string]$config.log_path
$exitPath = [string]$config.exit_path
$pidPath = [string]$config.pid_path
$testExecutable = [string]$config.test_executable
$exitTemp = "$exitPath.tmp-$PID"
$exitCode = 1
Set-Content -LiteralPath $pidPath -Value $PID -NoNewline

try {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $isAdministrator = $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator
    )
    if ($isAdministrator) {
        throw "Restricted browser-test child still has an active Administrators token"
    }
    "Restricted child verified (Administrators role inactive); running prebuilt test executable" |
        Add-Content -LiteralPath $logPath -Encoding UTF8

    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = @(& $testExecutable $testName `
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
} catch {
    ($_ | Out-String) | Add-Content -LiteralPath $logPath -Encoding UTF8
    $exitCode = 1
} finally {
    Set-Content -LiteralPath $exitTemp -Value $exitCode -NoNewline
    Move-Item -LiteralPath $exitTemp -Destination $exitPath
}

exit $exitCode
