# Setup CUA Computer Server on Windows 11
# Creates a scheduled task to run computer server in background

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

# --- Logging ---
$LogDir = "C:\Windows\Temp"
if (!(Test-Path $LogDir)) { New-Item -ItemType Directory -Force -Path $LogDir | Out-Null }
$RunId = (Get-Date -Format 'yyyyMMdd_HHmmss') + "_" + $PID
$script:LogFile = Join-Path $LogDir ("setup_cua_server_" + $RunId + ".log")
function Write-CuaSetupLog {
    param([string]$Message)
    $ts = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
    "$ts`t$Message" | Tee-Object -FilePath $script:LogFile -Append
    Write-Host "$ts`t$Message"
}

Write-CuaSetupLog "=== Installing CUA Computer Server ==="

function Resolve-ChocoPath {
  $inst = [Environment]::GetEnvironmentVariable('ChocolateyInstall','Machine')
  if ($inst) {
    $exe = Join-Path $inst 'bin\choco.exe'
    if (Test-Path $exe) { return $exe }
  }
  $fallback = 'C:\ProgramData\chocolatey\bin\choco.exe'
  if (Test-Path $fallback) { return $fallback }
  try {
    $cmd = (Get-Command choco -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source)
    if ($cmd) { return $cmd }
  } catch {}
  return $null
}

# Ensure Chocolatey and Python 3.12 are present
try {
  $ChocoExe = Resolve-ChocoPath
  if (-not $ChocoExe) {
    Write-CuaSetupLog "Installing Chocolatey"
    Set-ExecutionPolicy Bypass -Scope Process -Force
    [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.SecurityProtocolType]::Tls12
    Invoke-Expression ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))
    $ChocoExe = Resolve-ChocoPath
  }
  if ($ChocoExe) {
    Write-CuaSetupLog "Installing Python 3.12 via Chocolatey"
    try {
      & $ChocoExe install -y python312 | Out-Null
    } catch {
      Write-CuaSetupLog "Python 3.12 install warning: $($_.Exception.Message)"
    }
  } else {
    Write-CuaSetupLog "Chocolatey not available; skipping python312 install"
  }
} catch {
  Write-CuaSetupLog "Chocolatey bootstrap warning: $($_.Exception.Message)"
}

# Create venv
$HomeDir = $env:USERPROFILE
$CuaDir  = Join-Path $HomeDir '.cua-server'
$VenvDir = Join-Path $CuaDir 'venv'
New-Item -ItemType Directory -Force -Path $CuaDir | Out-Null

Write-CuaSetupLog "Creating Python virtual environment at $VenvDir"
$ExistingVenvPython = Join-Path $VenvDir 'Scripts\python.exe'
if (Test-Path -LiteralPath $ExistingVenvPython) {
  Write-CuaSetupLog "Existing venv detected; skipping creation"
} else {
  try {
    & py -m venv $VenvDir
    Write-CuaSetupLog "Virtual environment created successfully"
  } catch {
    Write-CuaSetupLog "venv creation error: $($_.Exception.Message)"
    throw
  }
}

$PyExe  = Join-Path $VenvDir 'Scripts\python.exe'
$PipExe = Join-Path $VenvDir 'Scripts\pip.exe'
$ActivateScript = Join-Path $VenvDir 'Scripts\Activate.ps1'

Write-CuaSetupLog "Activating virtual environment"
& $ActivateScript

Write-CuaSetupLog "Upgrading pip, setuptools, and wheel"
try {
  & $PipExe install --upgrade pip setuptools wheel 2>&1 | Tee-Object -FilePath $script:LogFile -Append | Out-Null
} catch {
  Write-CuaSetupLog "pip bootstrap warning: $($_.Exception.Message)"
}

Write-CuaSetupLog "Installing cua-computer-server"
try {
  & $PipExe install --upgrade cua-computer-server 2>&1 | Tee-Object -FilePath $script:LogFile -Append | Out-Null
  Write-CuaSetupLog "cua-computer-server installed successfully"
} catch {
  Write-CuaSetupLog "Server install error: $($_.Exception.Message)"
  throw
}

# Open firewall for port 5000
Write-CuaSetupLog "Opening firewall for port 5000"
try {
  netsh advfirewall firewall add rule name="CUA Computer Server 5000" dir=in action=allow protocol=TCP localport=5000 | Out-Null
  Write-CuaSetupLog "Firewall rule added successfully"
} catch {
  Write-CuaSetupLog "Firewall rule warning: $($_.Exception.Message)"
}

# Create start script with auto-restart
$StartScript = Join-Path $CuaDir 'start-server.ps1'
$StartScriptContent = @"
param()

`$env:PYTHONUNBUFFERED = '1'

`$LogFile = Join-Path '$CuaDir' 'server.log'
`$ActivateScript = '$ActivateScript'
`$PipExe = '$PipExe'
`$Python = '$PyExe'

function Start-Server {
    Write-Output "Activating virtual environment and updating cua-computer-server..." | Out-File -FilePath `$LogFile -Append
    & `$ActivateScript
    & `$PipExe install --upgrade cua-computer-server 2>&1 | Out-File -FilePath `$LogFile -Append

    Write-Output "Starting CUA Computer Server on port 5000..." | Out-File -FilePath `$LogFile -Append
    & `$Python -m computer_server --port 5000 2>&1 | Out-File -FilePath `$LogFile -Append
    return `$LASTEXITCODE
}

while (`$true) {
    Start-Server
    `$code = `$LASTEXITCODE
    Write-Output "Server exited with code: `$code. Restarting in 5s..." | Out-File -FilePath `$LogFile -Append
    Start-Sleep -Seconds 5
}
"@

Set-Content -Path $StartScript -Value $StartScriptContent -Encoding UTF8
Write-CuaSetupLog "Start script created at $StartScript"

# Create VBScript wrapper to launch PowerShell hidden
$VbsWrapper = Join-Path $CuaDir 'start-server-hidden.vbs'
$VbsContent = @"
Set objShell = CreateObject("WScript.Shell")
objShell.Run "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ""$StartScript""", 0, False
"@
Set-Content -Path $VbsWrapper -Value $VbsContent -Encoding ASCII
Write-CuaSetupLog "VBScript wrapper created at $VbsWrapper"

# Create scheduled task to run at logon
try {
  $TaskName = 'CUA-Computer-Server'
  $Username = 'Docker'  # Default user for Dockur Windows

  # Remove existing task if present
  $existingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($existingTask) {
    Write-CuaSetupLog "Removing existing scheduled task: $TaskName"
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
  Write-CuaSetupLog "Registering scheduled task '$TaskName' to run as $Username at logon (hidden)"
  Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $Action `
    -Trigger $Trigger `
    -Principal $Principal `
    -Settings $Settings `
    -Force | Out-Null

  Write-CuaSetupLog "Scheduled task '$TaskName' registered successfully (runs hidden in background)"

} catch {
  Write-CuaSetupLog "Scheduled task setup error: $($_.Exception.Message)"
  throw
}

Write-CuaSetupLog "=== CUA Computer Server setup completed ==="
exit 0
