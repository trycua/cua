# Unified CUA Windows Setup Script
#
# Environment Variables:
#   INSTALL_WINARENA_APPS - Set to "true" to install Windows Arena benchmark apps
#                          (Chrome, LibreOffice, VLC, GIMP, VS Code, Thunderbird, ffmpeg, 7zip)
#
# Always installs:
#   - Python 3.12 (via uv)
#   - uv (Python package manager)
#   - Git
#   - cua-computer-server
#   - Caddy Proxy (for reverse proxy)

$ErrorActionPreference = "Continue"

# ============================================================================
# Configuration
# ============================================================================

$scriptFolder = "\\host.lan\Data"
$toolsFolder = "C:\Users\$env:USERNAME\Tools"
$cuaServerPort = 5000
$onLogonTaskName = "CUA_OnLogon"

# Check if we should install Windows Arena apps
# First check environment variable, then check config file
$installWinarenaApps = $false
if ($env:INSTALL_WINARENA_APPS -eq "true") {
    $installWinarenaApps = $true
} else {
    # Check config file in shared folder
    $configFilePath = Join-Path $scriptFolder -ChildPath "install_config.json"
    if (Test-Path $configFilePath) {
        try {
            $config = Get-Content -Path $configFilePath -Raw | ConvertFrom-Json
            if ($config.INSTALL_WINARENA_APPS -eq $true) {
                $installWinarenaApps = $true
            }
        } catch {
            Write-Host "Warning: Could not read install_config.json"
        }
    }
}

Write-Host "============================================"
Write-Host "CUA Windows Setup"
Write-Host "============================================"
Write-Host "Install Windows Arena Apps: $installWinarenaApps"
Write-Host ""

# ============================================================================
# Load Modules and Initialize
# ============================================================================

# Load the shared setup-tools module
Import-Module (Join-Path $scriptFolder -ChildPath "setup-tools.psm1")

# Create a shortcut to the shared folder if it doesn't exist
$shortcutShared = "C:\Users\Docker\Desktop\Setup"
if (-not (Test-Path $shortcutShared)) {
    New-Item -ItemType SymbolicLink -Path $shortcutShared -Value $scriptFolder -ErrorAction SilentlyContinue
}

# Check if profile exists
if (-not (Test-Path $PROFILE)) {
    New-Item -ItemType File -Path $PROFILE -Force
}

# Create a folder where we store all the standalone executables
if (-not (Test-Path $toolsFolder)) {
    New-Item -ItemType Directory -Path $toolsFolder -Force
    $envPath = [Environment]::GetEnvironmentVariable("PATH", "Machine")
    $newPath = "$envPath;$toolsFolder"
    [Environment]::SetEnvironmentVariable("PATH", $newPath, "Machine")
}

# Set TLS version to 1.2 or higher
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13

# Load the tools config json
$toolsConfigJsonPath = Join-Path $scriptFolder -ChildPath "tools_config.json"
$toolsConfigJson = Get-Content -Path $toolsConfigJsonPath -Raw
$toolsList = Get-Tools -toolsConfigJson $toolsConfigJson

# ============================================================================
# Disable Windows Annoyances
# ============================================================================

Write-Host "Disabling Windows Backup and OneDrive prompts..."

# Disable Windows Backup via Registry
$backupRegPath = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\Backup"
if (-not (Test-Path $backupRegPath)) {
    New-Item -Path $backupRegPath -Force | Out-Null
}
$clientBackupPath = "$backupRegPath\Client"
if (-not (Test-Path $clientBackupPath)) {
    New-Item -Path $clientBackupPath -Force | Out-Null
}
Set-ItemProperty -Path $clientBackupPath -Name "DisableBackupUI" -Value 1 -Type DWord -Force

# Disable OneDrive
$oneDriveRegPath = "HKCU:\SOFTWARE\Microsoft\OneDrive"
if (-not (Test-Path $oneDriveRegPath)) {
    New-Item -Path $oneDriveRegPath -Force | Out-Null
}
Set-ItemProperty -Path $oneDriveRegPath -Name "DisableFileSyncNGSC" -Value 1 -Type DWord -Force

$oneDrivePolicyPath = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\OneDrive"
if (-not (Test-Path $oneDrivePolicyPath)) {
    New-Item -Path $oneDrivePolicyPath -Force | Out-Null
}
Set-ItemProperty -Path $oneDrivePolicyPath -Name "DisableFileSyncNGSC" -Value 1 -Type DWord -Force
Set-ItemProperty -Path $oneDrivePolicyPath -Name "DisableFileSync" -Value 1 -Type DWord -Force

# Disable notifications
$notificationPath = "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Notifications\Settings\Windows.SystemToast.BackupReminder"
if (-not (Test-Path $notificationPath)) {
    New-Item -Path $notificationPath -Force | Out-Null
}
Set-ItemProperty -Path $notificationPath -Name "Enabled" -Value 0 -Type DWord -Force

# Disable cloud content suggestions
$cloudContentPath = "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\ContentDeliveryManager"
if (Test-Path $cloudContentPath) {
    Set-ItemProperty -Path $cloudContentPath -Name "SubscribedContent-338389Enabled" -Value 0 -Type DWord -Force -ErrorAction SilentlyContinue
    Set-ItemProperty -Path $cloudContentPath -Name "SubscribedContent-310093Enabled" -Value 0 -Type DWord -Force -ErrorAction SilentlyContinue
    Set-ItemProperty -Path $cloudContentPath -Name "SoftLandingEnabled" -Value 0 -Type DWord -Force -ErrorAction SilentlyContinue
}

# Stop OneDrive
$oneDriveExe = "$env:LOCALAPPDATA\Microsoft\OneDrive\OneDrive.exe"
if (Test-Path $oneDriveExe) {
    Stop-Process -Name "OneDrive" -Force -ErrorAction SilentlyContinue
}
$startupRegPath = "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run"
Remove-ItemProperty -Path $startupRegPath -Name "OneDrive" -ErrorAction SilentlyContinue

Write-Host "Windows annoyances disabled."

# ============================================================================
# CORE: uv (Python package manager) - ALWAYS INSTALL
# ============================================================================

Write-Host ""
Write-Host "Installing uv (Python package manager)..."
$uvInstallerScript = "$env:TEMP\uv_installer.ps1"
Invoke-WebRequest -Uri "https://astral.sh/uv/install.ps1" -OutFile $uvInstallerScript
powershell -ExecutionPolicy Bypass -File $uvInstallerScript

$uvPath = "$env:USERPROFILE\.local\bin"
$env:PATH = "$uvPath;$env:PATH"
Add-ToEnvPath -NewPath $uvPath

$uvExePath = Join-Path $uvPath "uv.exe"
if (Test-Path $uvExePath) {
    Write-Host "[OK] uv installed at: $uvExePath"
} else {
    Write-Host "Waiting for uv installation..."
    Start-Sleep -Seconds 5
    if (-not (Test-Path $uvExePath)) {
        Write-Error "uv installation failed!"
    }
}

# ============================================================================
# CORE: Python 3.12 - ALWAYS INSTALL
# ============================================================================

Write-Host ""
Write-Host "Installing Python 3.12 via uv..."
uv python install 3.12

$pythonExecutablePath = (uv python find 3.12).Trim()
Write-Host "[OK] Python 3.12 at: $pythonExecutablePath"

$setAliasExpression = "Set-Alias -Name python -Value `"$pythonExecutablePath`""
Add-Content -Path $PROFILE -Value $setAliasExpression
Invoke-Expression $setAliasExpression

# ============================================================================
# CORE: Git - ALWAYS INSTALL
# ============================================================================

Write-Host ""
Write-Host "Installing Git..."
$gitToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "git"

try {
    git --version | Out-Null
    Write-Host "[OK] Git already installed."
} catch {
    $gitInstallerFilePath = "$env:TEMP\git_installer.exe"
    $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $gitToolDetails.mirrors -outfile $gitInstallerFilePath
    if ($downloadResult) {
        Start-Process -FilePath $gitInstallerFilePath -Args "/VERYSILENT /NORESTART /NOCANCEL /SP-" -Wait
        Add-ToEnvPath -NewPath "C:\Program Files\Git\bin"
        Write-Host "[OK] Git installed."
    } else {
        Write-Host "[WARN] Failed to download Git."
    }
}

# ============================================================================
# CORE: Caddy Proxy - ALWAYS INSTALL
# ============================================================================

Write-Host ""
Write-Host "Installing Caddy Proxy..."
$caddyToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "Caddy Proxy"
$caddyExecutablePath = "C:\Users\$env:USERNAME\caddy_windows_amd64.exe"

if (Test-Path $caddyExecutablePath) {
    Write-Host "[OK] Caddy already installed."
} else {
    $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $caddyToolDetails.mirrors -outfile $caddyExecutablePath
    if ($downloadResult) {
        $setAliasExpression = "Set-Alias -Name caddy -Value `"$caddyExecutablePath`""
        Add-Content -Path $PROFILE -Value $setAliasExpression
        Invoke-Expression $setAliasExpression
        Write-Host "[OK] Caddy installed."
    } else {
        Write-Host "[WARN] Failed to download Caddy."
    }
}

# ============================================================================
# CORE: cua-computer-server - ALWAYS INSTALL
# ============================================================================

Write-Host ""
Write-Host "Installing cua-computer-server..."
uv tool install cua-computer-server

$uvToolsBinPath = "$env:USERPROFILE\.local\bin"
Add-ToEnvPath -NewPath $uvToolsBinPath

# Firewall rules
$cuaServerRuleName = "CUAComputerServer-$cuaServerPort"
if (-not (Get-NetFirewallRule -Name $cuaServerRuleName -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $cuaServerRuleName -Direction Inbound -Protocol TCP -LocalPort $cuaServerPort -Action Allow -Profile Any
    Write-Host "[OK] Firewall rule added for port $cuaServerPort"
}

$caddyProxyRuleName = "Allow-Caddy-Proxy"
if (-not (Get-NetFirewallRule -Name $caddyProxyRuleName -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $caddyProxyRuleName -Direction Inbound -Program $caddyExecutablePath -Action Allow -Profile Any
    Write-Host "[OK] Firewall rule added for Caddy"
}

Write-Host "[OK] cua-computer-server installed."

# ============================================================================
# CORE: cua-bench-ui - ALWAYS INSTALL (for HTML window rendering)
# ============================================================================

Write-Host ""
Write-Host "Installing cua-bench-ui (for HTML window support)..."
# Install into the cua-computer-server's venv so it's available when tasks run
$cuaServerVenv = "$env:USERPROFILE\.cua-server\venv"
if (Test-Path "$cuaServerVenv\Scripts\pip.exe") {
    & "$cuaServerVenv\Scripts\pip.exe" install cua-bench-ui
    Write-Host "[OK] cua-bench-ui installed in cua-server venv."
} else {
    # Fallback: install globally via pip
    pip install cua-bench-ui
    Write-Host "[OK] cua-bench-ui installed globally."
}

# ============================================================================
# OPTIONAL: Windows Arena Benchmark Apps
# ============================================================================

if ($installWinarenaApps) {
    Write-Host ""
    Write-Host "============================================"
    Write-Host "Installing Windows Arena Benchmark Apps..."
    Write-Host "============================================"

    # --- 7zip ---
    Write-Host ""
    Write-Host "Installing 7-Zip..."
    $7ZipToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "7zip"
    if (Get-Command 7z -ErrorAction SilentlyContinue) {
        Write-Host "[OK] 7-Zip already installed."
    } else {
        $7ZipInstallerFilePath = "$env:TEMP\7_zip.exe"
        $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $7ZipToolDetails.mirrors -outfile $7ZipInstallerFilePath
        if ($downloadResult) {
            Start-Process -FilePath $7ZipInstallerFilePath -Args "/S" -Verb RunAs -Wait
            Remove-Item $7ZipInstallerFilePath -ErrorAction SilentlyContinue
            Add-ToEnvPath -NewPath "${env:ProgramFiles}\7-Zip"
            Write-Host "[OK] 7-Zip installed."
        }
    }

    # --- ffmpeg ---
    Write-Host ""
    Write-Host "Installing ffmpeg..."
    $ffmpegToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "ffmpeg"
    if (Get-Command ffmpeg -ErrorAction SilentlyContinue) {
        Write-Host "[OK] ffmpeg already installed."
    } else {
        $ffmpegInstallerFilePath = "C:\ffmpeg.7z"
        $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $ffmpegToolDetails.mirrors -outfile $ffmpegInstallerFilePath
        if ($downloadResult) {
            7z x -y -o"C:\" "C:\ffmpeg.7z"
            $ffmpegFolder = Get-ChildItem -Path "C:\" -Filter "ffmpeg-*" -Directory
            $ffmpegFolder = -join ("C:\", $ffmpegFolder)
            if (Test-Path "C:\ffmpeg") {
                Remove-Item -Path "C:\ffmpeg" -Recurse -Force
            }
            Rename-Item -Path "$ffmpegFolder" -NewName "ffmpeg"
            Add-ToEnvPath -NewPath "C:\ffmpeg\bin"
            Write-Host "[OK] ffmpeg installed."
        }
    }

    # Disable Edge Auto Updates
    Stop-Process -Name "MicrosoftEdgeUpdate" -Force -ErrorAction SilentlyContinue
    $edgeUpdatePath = "${env:ProgramFiles(x86)}\Microsoft\EdgeUpdate"
    Remove-Item -Path $edgeUpdatePath -Recurse -Force -ErrorAction SilentlyContinue

    # --- Google Chrome ---
    Write-Host ""
    Write-Host "Installing Google Chrome..."
    $chromeToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "Google Chrome"
    $chromeExePath = "C:\Program Files\Google\Chrome\Application\chrome.exe"
    if (Test-Path $chromeExePath) {
        Write-Host "[OK] Chrome already installed."
    } else {
        $chromeInstallerFilePath = "$env:TEMP\chrome_installer.exe"
        $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $chromeToolDetails.mirrors -outfile $chromeInstallerFilePath
        if ($downloadResult) {
            Start-Process -FilePath $chromeInstallerFilePath -ArgumentList "/silent", "/install" -Verb RunAs -Wait
            Remove-Item -Path $chromeInstallerFilePath -ErrorAction SilentlyContinue
            Add-ToEnvPath -NewPath "${env:ProgramFiles}\Google\Chrome\Application"
            # Disable auto updates
            $chromeRegPath = "HKLM:\SOFTWARE\Policies\Google\Update"
            if (-not (Test-Path $chromeRegPath)) { New-Item -Path $chromeRegPath -Force }
            Set-ItemProperty -Path $chromeRegPath -Name "AutoUpdateCheckPeriodMinutes" -Value 0
            Set-ItemProperty -Path $chromeRegPath -Name "UpdateDefault" -Value 0
            Write-Host "[OK] Chrome installed."
        }
    }

    # --- LibreOffice ---
    Write-Host ""
    Write-Host "Installing LibreOffice..."
    $libreOfficeToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "LibreOffice"
    $installedVersion = (Get-WmiObject -Query "SELECT * FROM Win32_Product WHERE Name like 'LibreOffice%'").Version
    if (-not [string]::IsNullOrWhiteSpace($installedVersion)) {
        Write-Host "[OK] LibreOffice already installed."
    } else {
        $libreOfficeInstallerFilePath = "$env:TEMP\libreOffice_installer.msi"
        $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $libreOfficeToolDetails.mirrors -outfile $libreOfficeInstallerFilePath
        if ($downloadResult) {
            Start-Process "msiexec.exe" -ArgumentList "/i `"$libreOfficeInstallerFilePath`" /quiet" -Wait -NoNewWindow
            Add-ToEnvPath -NewPath "C:\Program Files\LibreOffice\program"
            Write-Host "[OK] LibreOffice installed."
        }
    }

    # --- VLC ---
    Write-Host ""
    Write-Host "Installing VLC..."
    $vlcToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "VLC"
    $vlcExecutableFilePath = "C:\Program Files\VideoLAN\VLC\vlc.exe"
    if (Test-Path $vlcExecutableFilePath) {
        Write-Host "[OK] VLC already installed."
    } else {
        $vlcInstallerFilePath = "$env:TEMP\vlc_installer.exe"
        $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $vlcToolDetails.mirrors -outfile $vlcInstallerFilePath
        if ($downloadResult) {
            Start-Process -FilePath $vlcInstallerFilePath -ArgumentList "/S" -Verb RunAs -Wait
            Remove-Item -Path $vlcInstallerFilePath -ErrorAction SilentlyContinue
            Add-ToEnvPath -NewPath "C:\Program Files\VideoLAN\VLC"
            Write-Host "[OK] VLC installed."
        }
    }

    # --- GIMP ---
    Write-Host ""
    Write-Host "Installing GIMP..."
    $gimpToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "GIMP"
    $gimpExecutablePath = "C:\Program Files\GIMP 2\bin\gimp-2.10.exe"
    if (Test-Path $gimpExecutablePath) {
        Write-Host "[OK] GIMP already installed."
    } else {
        $gimpInstallerFilePath = "$env:TEMP\gimp_installer.exe"
        $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $gimpToolDetails.mirrors -outfile $gimpInstallerFilePath
        if ($downloadResult) {
            Start-Process -FilePath $gimpInstallerFilePath -ArgumentList "/VERYSILENT /ALLUSERS" -Verb RunAs -Wait
            Remove-Item -Path $gimpInstallerFilePath -ErrorAction SilentlyContinue
            Add-ToEnvPath -NewPath "C:\Program Files\GIMP 2\bin"
            Write-Host "[OK] GIMP installed."
        }
    }

    # --- VS Code ---
    Write-Host ""
    Write-Host "Installing VS Code..."
    $vsCodeToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "VS Code"
    $vsCodeExecutablePath = "C:\Users\$env:USERNAME\AppData\Local\Programs\Microsoft VS Code\Code.exe"
    if (Test-Path $vsCodeExecutablePath) {
        Write-Host "[OK] VS Code already installed."
    } else {
        $vsCodeInstallerFilePath = "$env:TEMP\VSCodeSetup.exe"
        $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $vsCodeToolDetails.mirrors -outfile $vsCodeInstallerFilePath
        if ($downloadResult) {
            Start-Process -FilePath $vsCodeInstallerFilePath -ArgumentList "/VERYSILENT", "/mergetasks=!runcode" -Verb RunAs -Wait
            Remove-Item -Path $vsCodeInstallerFilePath -ErrorAction SilentlyContinue
            Add-ToEnvPath -NewPath "C:\Users\$env:USERNAME\AppData\Local\Programs\Microsoft VS Code\bin"
            # Disable auto updates
            $vsCodeSettingsPath = "${env:APPDATA}\Code\User\settings.json"
            $dirPath = Split-Path -Path $vsCodeSettingsPath -Parent
            if (-not (Test-Path $dirPath)) {
                New-Item -ItemType Directory -Path $dirPath -Force
            }
            @{ "update.mode" = "none" } | ConvertTo-Json | Set-Content $vsCodeSettingsPath
            Write-Host "[OK] VS Code installed."
        }
    }

    # --- Thunderbird ---
    Write-Host ""
    Write-Host "Installing Thunderbird..."
    $thunderbirdToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "Thunderbird"
    $thunderbirdExecutablePath = "C:\Program Files\Mozilla Thunderbird\thunderbird.exe"
    if (Test-Path $thunderbirdExecutablePath) {
        Write-Host "[OK] Thunderbird already installed."
    } else {
        $thunderbirdInstallerFilePath = "$env:TEMP\ThunderbirdSetup.exe"
        $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $thunderbirdToolDetails.mirrors -outfile $thunderbirdInstallerFilePath
        if ($downloadResult) {
            Start-Process -FilePath $thunderbirdInstallerFilePath -ArgumentList "/S" -Verb RunAs -Wait
            Remove-Item -Path $thunderbirdInstallerFilePath -ErrorAction SilentlyContinue
            Add-ToEnvPath -NewPath "C:\Program Files\Mozilla Thunderbird"
            Write-Host "[OK] Thunderbird installed."
        }
    }

    Write-Host ""
    Write-Host "[OK] Windows Arena apps installation complete."
}

# ============================================================================
# Register On-Logon Task (starts cua-computer-server on boot)
# ============================================================================

Write-Host ""
Write-Host "Registering on-logon task..."

$onLogonScriptPath = "$scriptFolder\on-logon.ps1"
if (Get-ScheduledTask -TaskName $onLogonTaskName -ErrorAction SilentlyContinue) {
    Write-Host "[OK] Scheduled task already exists."
} else {
    Register-LogonTask -TaskName $onLogonTaskName -ScriptPath $onLogonScriptPath -LocalUser "Docker"
    Write-Host "[OK] Scheduled task registered."
}

Start-Sleep -Seconds 5
Start-ScheduledTask -TaskName $onLogonTaskName

Write-Host ""
Write-Host "============================================"
Write-Host "CUA Windows Setup Complete!"
Write-Host "============================================"
