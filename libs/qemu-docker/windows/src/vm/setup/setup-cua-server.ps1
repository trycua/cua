# Setup Cua Computer Server on Windows 11
# Creates a scheduled task to run computer server in background

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

# Import shared utilities
$scriptFolder = "C:\OEM"
Import-Module (Join-Path $scriptFolder -ChildPath "setup-utils.psm1")

# --- Logging ---
$LogDir = "C:\Windows\Temp"
if (!(Test-Path $LogDir)) { New-Item -ItemType Directory -Force -Path $LogDir | Out-Null }
$RunId = (Get-Date -Format 'yyyyMMdd_HHmmss') + "_" + $PID
$script:LogFile = Join-Path $LogDir ("setup_cua_server_" + $RunId + ".log")

Write-Log -LogFile $script:LogFile -Message "=== Installing Cua Computer Server ==="

# Ensure Chocolatey and Python 3.12 are present
try {
  $ChocoExe = Resolve-ChocoPath
  if ($ChocoExe) {
    Write-Log -LogFile $script:LogFile -Message "Installing Python 3.12 via Chocolatey"
    try {
      & $ChocoExe install -y python312 | Out-Null
    } catch {
      Write-Log -LogFile $script:LogFile -Message "Python 3.12 install warning: $($_.Exception.Message)"
    }
  } else {
    Write-Log -LogFile $script:LogFile -Message "Chocolatey not available; skipping python312 install"
  }
} catch {
  Write-Log -LogFile $script:LogFile -Message "Chocolatey bootstrap warning: $($_.Exception.Message)"
}

# Install UV package manager first (with retry logic)
Write-Log -LogFile $script:LogFile -Message "Installing UV package manager"
$maxRetries = 3
$retryDelay = 5
$uvInstalled = $false

for ($i = 1; $i -le $maxRetries; $i++) {
  try {
    Write-Log -LogFile $script:LogFile -Message "UV installation attempt $i of $maxRetries"
    # Install UV using the official standalone installer
    powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex" 2>&1 | Tee-Object -FilePath $script:LogFile -Append | Out-Null

    # Add UV to PATH for current session
    $uvPath = Join-Path $env:USERPROFILE ".local\bin"
    if (Test-Path $uvPath) {
      $env:Path = "$uvPath;$env:Path"
      Write-Log -LogFile $script:LogFile -Message "UV installed successfully and added to PATH"
      $uvInstalled = $true
      break
    } else {
      Write-Log -LogFile $script:LogFile -Message "UV installed but path not found at $uvPath"
      $uvInstalled = $true
      break
    }
  } catch {
    Write-Log -LogFile $script:LogFile -Message "UV install attempt $i failed: $($_.Exception.Message)"
    if ($i -lt $maxRetries) {
      Write-Log -LogFile $script:LogFile -Message "Retrying in $retryDelay seconds..."
      Start-Sleep -Seconds $retryDelay
      $retryDelay = $retryDelay * 2  # Exponential backoff
    }
  }
}

if (-not $uvInstalled) {
  Write-Log -LogFile $script:LogFile -Message "UV installation failed after $maxRetries attempts"
  throw "Failed to install UV package manager"
}

# Create UV project directory
$HomeDir = $env:USERPROFILE
$ProjectDir = Join-Path $HomeDir 'cua-server'

Write-Log -LogFile $script:LogFile -Message "Creating UV project at $ProjectDir"
$ProjectFile = Join-Path $ProjectDir 'pyproject.toml'
if (Test-Path -LiteralPath $ProjectFile) {
  Write-Log -LogFile $script:LogFile -Message "Existing UV project detected; skipping creation"
} else {
  try {
    # Create parent directory if it doesn't exist
    New-Item -ItemType Directory -Force -Path $ProjectDir | Out-Null
    & uv init --vcs none --no-readme --no-workspace --no-pin-python $ProjectDir 2>&1 | Tee-Object -FilePath $script:LogFile -Append | Out-Null
    Write-Log -LogFile $script:LogFile -Message "UV project created successfully"
  } catch {
    Write-Log -LogFile $script:LogFile -Message "UV project creation error: $($_.Exception.Message)"
    throw
  }
}

Write-Log -LogFile $script:LogFile -Message "Installing cua-computer-server"
try {
  & uv add --directory $ProjectDir cua-computer-server 2>&1 | Tee-Object -FilePath $script:LogFile -Append | Out-Null
  Write-Log -LogFile $script:LogFile -Message "cua-computer-server installed successfully"
} catch {
  Write-Log -LogFile $script:LogFile -Message "Server install error: $($_.Exception.Message)"
  throw
}

Write-Log -LogFile $script:LogFile -Message "Installing playwright & firefox browser"
try {
  & uv add --directory $ProjectDir playwright 2>&1 | Tee-Object -FilePath $script:LogFile -Append | Out-Null
  & uv run --directory $ProjectDir playwright install firefox 2>&1 | Tee-Object -FilePath $script:LogFile -Append | Out-Null
  Write-Log -LogFile $script:LogFile -Message "playwright installed successfully"
} catch {
  Write-Log -LogFile $script:LogFile -Message "Playwright install error: $($_.Exception.Message)"
  throw
}

# Open firewall for port 5000
Write-Log -LogFile $script:LogFile -Message "Opening firewall for port 5000"
try {
  netsh advfirewall firewall add rule name="Cua Computer Server 5000" dir=in action=allow protocol=TCP localport=5000 | Out-Null
  Write-Log -LogFile $script:LogFile -Message "Firewall rule added successfully"
} catch {
  Write-Log -LogFile $script:LogFile -Message "Firewall rule warning: $($_.Exception.Message)"
}

# Create start script with auto-restart
$StartScript = Join-Path $ProjectDir 'start-server.ps1'
$StartScriptContent = @"
param()

`$env:PYTHONUNBUFFERED = '1'

# Ensure UV is in PATH
`$uvPath = Join-Path `$env:USERPROFILE '.local\bin'
if (Test-Path `$uvPath) {
    `$env:Path = "`$uvPath;`$env:Path"
}

`$ProjectDir = '$ProjectDir'
`$LogFile = Join-Path `$ProjectDir 'server.log'

function Start-Server {
    Write-Output "Updating cua-computer-server..." | Out-File -FilePath `$LogFile -Append
    & uv add --directory `$ProjectDir cua-computer-server 2>&1 | Out-File -FilePath `$LogFile -Append

    Write-Output "Starting Cua Computer Server on port 5000..." | Out-File -FilePath `$LogFile -Append
    & uv run --directory `$ProjectDir python -m computer_server --port 5000 2>&1 | Out-File -FilePath `$LogFile -Append
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
Write-Log -LogFile $script:LogFile -Message "Start script created at $StartScript"

# Create VBScript wrapper to launch PowerShell hidden
$VbsWrapper = Join-Path $ProjectDir 'start-server-hidden.vbs'
$VbsContent = @"
Set objShell = CreateObject("WScript.Shell")
objShell.Run "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ""$StartScript""", 0, False
"@
Set-Content -Path $VbsWrapper -Value $VbsContent -Encoding ASCII
Write-Log -LogFile $script:LogFile -Message "VBScript wrapper created at $VbsWrapper"

# Create scheduled task to run at logon
try {
  $TaskName = 'Cua-Computer-Server'
  $Username = 'Docker'  # Default user for Dockur Windows

  # Remove existing task if present
  $existingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($existingTask) {
    Write-Log -LogFile $script:LogFile -Message "Removing existing scheduled task: $TaskName"
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
  }

  # Create action to run VBScript wrapper (hidden)
  $Action = New-ScheduledTaskAction -Execute 'wscript.exe' -Argument "`"$VbsWrapper`""

  # Trigger: At logon of user
  $UserId = "$env:COMPUTERNAME\$Username"
  $Trigger = New-ScheduledTaskTrigger -AtLogOn -User $UserId

  # Principal: Run in user's interactive session (required for screen capture)
  $Principal = New-ScheduledTaskPrincipal -UserId $UserId -LogonType Interactive -RunLevel Highest

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
  Write-Log -LogFile $script:LogFile -Message "Registering scheduled task '$TaskName' to run as $Username at logon (hidden)"
  Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $Action `
    -Trigger $Trigger `
    -Principal $Principal `
    -Settings $Settings `
    -Force | Out-Null

  Write-Log -LogFile $script:LogFile -Message "Scheduled task '$TaskName' registered successfully (runs hidden in background)"

} catch {
  Write-Log -LogFile $script:LogFile -Message "Scheduled task setup error: $($_.Exception.Message)"
  throw
}

Write-Log -LogFile $script:LogFile -Message "=== Cua Computer Server setup completed ==="
exit 0
