# Run the jev-use deterministic proof against the installed Cua Driver from a
# standard-user token. Start this through invoke-standard-user-token.ps1; it
# refuses to run with administrator rights, because Driver correctly refuses
# isolated browser launch from an installation the current token can modify.
#
# Inputs (environment):
#   CUA_DRIVER_BIN      installed cua-driver.exe
#   JEV_USE_PROOF_DIR   new evidence directory for verify_setup.py
#   JEV_USE_LOG_DIR     existing directory for daemon and token diagnostics
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$driver = $env:CUA_DRIVER_BIN
$proofDir = $env:JEV_USE_PROOF_DIR
$logDir = $env:JEV_USE_LOG_DIR
foreach ($name in @("CUA_DRIVER_BIN", "JEV_USE_PROOF_DIR", "JEV_USE_LOG_DIR")) {
    if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name))) {
        throw "$name is required"
    }
}
if (-not (Test-Path -LiteralPath $driver -PathType Leaf)) {
    throw "installed Driver not found: $driver"
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$exampleDir = Join-Path $repoRoot "libs\cua-driver\examples\jev-use"
$python = Join-Path $exampleDir ".venv\Scripts\python.exe"
if (-not (Test-Path -LiteralPath $python -PathType Leaf)) {
    throw "locked example environment not found: $python"
}

# Prove the token is a standard-user token before touching the Driver.
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
$groups = (& whoami.exe /groups /fo csv | ConvertFrom-Csv)
$groups | Format-Table -AutoSize | Out-String -Width 400 |
    Set-Content -Encoding utf8 (Join-Path $logDir "token-groups.txt")
$administrators = @($groups | Where-Object { $_.SID -eq "S-1-5-32-544" })
Write-Host "Token user: $($identity.Name); session: $((Get-Process -Id $PID).SessionId)"
Write-Host "Administrators group: $(if ($administrators) { $administrators[0].Attributes } else { 'absent' })"
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "the proof must run from a standard-user token, but Administrators is enabled"
}
if ($administrators -and $administrators[0].Attributes -notmatch "deny only") {
    throw "Administrators is present without the deny-only attribute: $($administrators[0].Attributes)"
}

$daemon = $null
try {
    Write-Host "[driver] $(& $driver --version)"
    # The installer ran with -NoAutoStart, so no elevated daemon exists. Start
    # the daemon from this token, as a standard user's logon task would.
    $daemon = Start-Process -FilePath $driver -ArgumentList "serve" -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $logDir "daemon.out.log") `
        -RedirectStandardError (Join-Path $logDir "daemon.err.log")
    $deadline = (Get-Date).AddSeconds(60)
    while ($true) {
        & $driver status *> (Join-Path $logDir "daemon-status.txt")
        if ($LASTEXITCODE -eq 0) { break }
        if ($daemon.HasExited) { throw "Driver daemon exited early with code $($daemon.ExitCode)" }
        if ((Get-Date) -gt $deadline) { throw "Driver daemon did not become ready within 60 seconds" }
        Start-Sleep -Milliseconds 500
    }
    Get-Content (Join-Path $logDir "daemon-status.txt")

    Push-Location $exampleDir
    try {
        & $python verify_mcp_tools.py
        if ($LASTEXITCODE -ne 0) { throw "verify_mcp_tools.py failed with exit code $LASTEXITCODE" }
        & $python verify_setup.py --typescript --output-dir $proofDir
        if ($LASTEXITCODE -ne 0) { throw "verify_setup.py failed with exit code $LASTEXITCODE" }
    } finally {
        Pop-Location
    }
} finally {
    & $driver stop *> $null
    if ($null -ne $daemon -and -not $daemon.HasExited) {
        Stop-Process -Id $daemon.Id -Force -ErrorAction SilentlyContinue
    }
}
