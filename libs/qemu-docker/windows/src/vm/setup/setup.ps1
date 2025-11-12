$ErrorActionPreference = "Continue"

$scriptFolder = "C:\OEM"

Import-Module (Join-Path $scriptFolder -ChildPath "setup-utils.psm1")

Set-StrictMode -Version Latest

# Set TLS version to 1.2 or higher
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13

# Install Git via Chocolatey
$ChocoExe = Resolve-ChocoPath
if ($ChocoExe) {
    Write-Host "Installing Git via Chocolatey..."
    try {
        & $ChocoExe install -y git | Out-Null
        Add-ToEnvPath -NewPath "C:\Program Files\Git\bin"
        Write-Host "Git installed successfully."
    } catch {
        Write-Host "Git install warning: $($_.Exception.Message)"
    }
} else {
    Write-Host "Chocolatey not available; skipping Git install"
}

# CUA Computer Server Setup
Write-Host "Setting up CUA Computer Server..."
$cuaServerSetupScript = Join-Path $scriptFolder -ChildPath "setup-cua-server.ps1"
if (Test-Path $cuaServerSetupScript) {
    & $cuaServerSetupScript
    Write-Host "CUA Computer Server setup completed."
} else {
    Write-Host "ERROR: setup-cua-server.ps1 not found at $cuaServerSetupScript"
}

# Register on-logon task
$onLogonTaskName = "WindowsArena_OnLogon"
$onLogonScriptPath = "$scriptFolder\on-logon.ps1"
if (Get-ScheduledTask -TaskName $onLogonTaskName -ErrorAction SilentlyContinue) {
    Write-Host "Scheduled task $onLogonTaskName already exists."
} else {
    Write-Host "Registering new task $onLogonTaskName..."
    Register-LogonTask -TaskName $onLogonTaskName -ScriptPath $onLogonScriptPath -LocalUser "Docker"
}

Start-Sleep -Seconds 10
Write-Host "Starting $onLogonTaskName task in background..."
Start-Process -WindowStyle Hidden -FilePath "powershell.exe" -ArgumentList "-Command", "Start-ScheduledTask -TaskName '$onLogonTaskName'"