# Setup cua-spacesd on Windows 11
# Installs the in-sandbox daemon and a scheduled task that serves gRPC +
# gRPC-Web on 0.0.0.0:3211 in the logged-on user's interactive session.
#
# Release artifact (placeholder until the Windows build is published by the
# cua-spacesd release workflow; mirrors the Linux tarball naming used by
# libs/cua-spacesd/packaging/install.sh):
#   https://github.com/trycua/cua/releases/download/cua-spacesd-v<VERSION>/cua-spacesd-x86_64-pc-windows-msvc.zip
#
# Token: CUA_ENV_TOKEN, else C:\ProgramData\cua\env-token (copied from
# C:\OEM\env-token when provided, otherwise generated). The token reaches the
# driver through the environment, never the command line.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$scriptFolder = "C:\OEM"
Import-Module (Join-Path $scriptFolder -ChildPath "setup-utils.psm1")

$LogDir = "C:\Windows\Temp"
if (!(Test-Path $LogDir)) { New-Item -ItemType Directory -Force -Path $LogDir | Out-Null }
$RunId = (Get-Date -Format 'yyyyMMdd_HHmmss') + "_" + $PID
$script:LogFile = Join-Path $LogDir ("setup_spacesd_" + $RunId + ".log")

$Version = if ($env:CUA_SPACESD_VERSION) { $env:CUA_SPACESD_VERSION } elseif ($env:CUA_GUESTD_VERSION) { $env:CUA_GUESTD_VERSION } else { '0.1.0' }
$Port = if ($env:CUA_ENV_PORT) { $env:CUA_ENV_PORT } else { '3211' }
$InstallDir = 'C:\Program Files\cua-spacesd'
$DataDir = 'C:\ProgramData\cua'
$TokenFile = Join-Path $DataDir 'env-token'
$Url = "https://github.com/trycua/cua/releases/download/cua-spacesd-v$Version/cua-spacesd-x86_64-pc-windows-msvc.zip"

Write-Log -LogFile $script:LogFile -Message "=== Installing cua-spacesd $Version ==="

New-Item -ItemType Directory -Force -Path $InstallDir, $DataDir | Out-Null
$zip = Join-Path $env:TEMP 'cua-spacesd.zip'
$installed = $false
for ($i = 1; $i -le 5; $i++) {
  try {
    Invoke-WebRequest -Uri $Url -OutFile $zip -UseBasicParsing
    Expand-Archive -Path $zip -DestinationPath $InstallDir -Force
    $installed = $true
    break
  } catch {
    Write-Log -LogFile $script:LogFile -Message "Download attempt $i failed: $($_.Exception.Message)"
    Start-Sleep -Seconds ($i * 5)
  }
}
if (-not $installed) { throw "Failed to download $Url" }
$Exe = Join-Path $InstallDir 'cua-spacesd.exe'
Write-Log -LogFile $script:LogFile -Message "cua-spacesd installed at $Exe"

# Persistent token
$oemToken = Join-Path $scriptFolder 'env-token'
if (Test-Path $oemToken) {
  Copy-Item $oemToken $TokenFile -Force
} elseif (!(Test-Path $TokenFile)) {
  $bytes = New-Object byte[] 32
  [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
  Set-Content -Path $TokenFile -Value (($bytes | ForEach-Object { $_.ToString('x2') }) -join '') -NoNewline -Encoding ASCII
}
Write-Log -LogFile $script:LogFile -Message "Token stored at $TokenFile"

try {
  netsh advfirewall firewall add rule name="cua-spacesd $Port" dir=in action=allow protocol=TCP localport=$Port | Out-Null
} catch {
  Write-Log -LogFile $script:LogFile -Message "Firewall rule warning: $($_.Exception.Message)"
}

$StartScript = Join-Path $InstallDir 'start-spacesd.ps1'
$StartScriptContent = @"
param()
`$token = `$env:CUA_ENV_TOKEN
if (-not `$token) { `$token = (Get-Content -Raw '$TokenFile').Trim() }
if (-not `$token) { throw 'no cua-spacesd token' }
`$env:CUA_ENV_TOKEN = `$token
while (`$true) {
    & '$Exe' --listen '0.0.0.0:$Port'
    Start-Sleep -Seconds 5
}
"@
Set-Content -Path $StartScript -Value $StartScriptContent -Encoding UTF8

$VbsWrapper = Join-Path $InstallDir 'start-spacesd-hidden.vbs'
$VbsContent = @"
Set objShell = CreateObject("WScript.Shell")
objShell.Run "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ""$StartScript""", 0, False
"@
Set-Content -Path $VbsWrapper -Value $VbsContent -Encoding ASCII

try {
  $TaskName = 'Cua-Spacesd'
  $Username = 'Docker'  # Default user for Dockur Windows
  $existingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($existingTask) { Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false }

  $Action = New-ScheduledTaskAction -Execute 'wscript.exe' -Argument "`"$VbsWrapper`""
  $UserId = "$env:COMPUTERNAME\$Username"
  $Trigger = New-ScheduledTaskTrigger -AtLogOn -User $UserId
  # Interactive session is required for screen capture and input
  $Principal = New-ScheduledTaskPrincipal -UserId $UserId -LogonType Interactive -RunLevel Highest
  $Settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -RestartCount 999 `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit (New-TimeSpan -Days 365) `
    -Hidden
  Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger `
    -Principal $Principal -Settings $Settings -Force | Out-Null
  Write-Log -LogFile $script:LogFile -Message "Scheduled task '$TaskName' registered"
} catch {
  Write-Log -LogFile $script:LogFile -Message "Scheduled task setup error: $($_.Exception.Message)"
  throw
}

Write-Log -LogFile $script:LogFile -Message "=== cua-spacesd setup completed ==="
exit 0
