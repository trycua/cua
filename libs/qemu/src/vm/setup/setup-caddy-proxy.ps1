# Setup Caddy Reverse Proxy on Windows 11
# Creates a scheduled task to run Caddy in background

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

# --- Logging ---
$LogDir = "C:\Windows\Temp"
if (!(Test-Path $LogDir)) { New-Item -ItemType Directory -Force -Path $LogDir | Out-Null }
$RunId = (Get-Date -Format 'yyyyMMdd_HHmmss') + "_" + $PID
$script:LogFile = Join-Path $LogDir ("setup_caddy_proxy_" + $RunId + ".log")
function Write-CaddySetupLog {
    param([string]$Message)
    $ts = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
    "$ts`t$Message" | Tee-Object -FilePath $script:LogFile -Append
    Write-Host "$ts`t$Message"
}

Write-CaddySetupLog "=== Setting up Caddy Reverse Proxy ==="

# Create directory for Caddy
$HomeDir = $env:USERPROFILE
$CaddyDir  = Join-Path $HomeDir '.caddy-proxy'
New-Item -ItemType Directory -Force -Path $CaddyDir | Out-Null

# Create start script for Caddy
$StartScript = Join-Path $CaddyDir 'start-caddy.ps1'
$CaddyExePath = "C:\Users\$env:USERNAME\caddy_windows_amd64.exe"
$StartScriptContent = @"
param()

`$LogFile = Join-Path '$CaddyDir' 'caddy.log'
`$CaddyExe = '$CaddyExePath'

while (`$true) {
    Write-Output "Starting Caddy reverse proxy from port 9222 to port 1337..." | Out-File -FilePath `$LogFile -Append
    & `$CaddyExe reverse-proxy --from :9222 --to :1337 2>&1 | Out-File -FilePath `$LogFile -Append
    `$code = `$LASTEXITCODE
    Write-Output "Caddy exited with code: `$code. Restarting in 5s..." | Out-File -FilePath `$LogFile -Append
    Start-Sleep -Seconds 5
}
"@

Set-Content -Path $StartScript -Value $StartScriptContent -Encoding UTF8
Write-CaddySetupLog "Start script created at $StartScript"

# Create VBScript wrapper to launch PowerShell hidden
$VbsWrapper = Join-Path $CaddyDir 'start-caddy-hidden.vbs'
$VbsContent = @"
Set objShell = CreateObject("WScript.Shell")
objShell.Run "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ""$StartScript""", 0, False
"@
Set-Content -Path $VbsWrapper -Value $VbsContent -Encoding ASCII
Write-CaddySetupLog "VBScript wrapper created at $VbsWrapper"

# Create scheduled task to run at logon
try {
  $TaskName = 'Caddy-Reverse-Proxy'
  $Username = 'Docker'  # Default user for Dockur Windows

  # Remove existing task if present
  $existingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($existingTask) {
    Write-CaddySetupLog "Removing existing scheduled task: $TaskName"
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
  }

  # Create action to run VBScript wrapper (hidden)
  $Action = New-ScheduledTaskAction -Execute 'wscript.exe' -Argument "`"$VbsWrapper`""

  # Trigger: At logon of user
  $UserId = "$env:COMPUTERNAME\$Username"
  $Trigger = New-ScheduledTaskTrigger -AtLogOn -User $UserId

  # Principal: Run in background without window (S4U = Service For User)
  $Principal = New-ScheduledTaskPrincipal -UserId $UserId -LogonType S4U -RunLevel Highest

  # Task settings - hide window
  $Settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -RestartCount 999 `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit (New-TimeSpan -Days 365) `
    -Hidden

  # Register the task
  Write-CaddySetupLog "Registering scheduled task '$TaskName' to run as $Username at logon (hidden)"
  Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $Action `
    -Trigger $Trigger `
    -Principal $Principal `
    -Settings $Settings `
    -Force | Out-Null

  Write-CaddySetupLog "Scheduled task '$TaskName' registered successfully (runs hidden in background)"

} catch {
  Write-CaddySetupLog "Scheduled task setup error: $($_.Exception.Message)"
  throw
}

Write-CaddySetupLog "=== Caddy Reverse Proxy setup completed ==="
exit 0
