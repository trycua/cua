$ErrorActionPreference = "Continue" # until downloading from mirrors is more stable

# Section - General Setup
$shortcutShared = "C:\Users\Docker\Desktop\Setup"
$scriptFolder = "\\host.lan\Data"
$toolsFolder = "C:\Users\$env:USERNAME\Tools"

# Check if we should install Windows Arena apps (Chrome, LibreOffice, VLC, etc.)
# Default to true for backwards compatibility
$installWinarenaApps = $true
$configFilePath = Join-Path $scriptFolder -ChildPath "install_config.json"
if (Test-Path $configFilePath) {
    try {
        $config = Get-Content -Path $configFilePath -Raw | ConvertFrom-Json
        if ($config.INSTALL_WINARENA_APPS -eq $false) {
            $installWinarenaApps = $false
        }
    } catch {
        Write-Host "Warning: Could not read install_config.json, defaulting to full install"
    }
}

Write-Host "============================================"
Write-Host "CUA Windows Setup"
Write-Host "============================================"
Write-Host "Install Windows Arena Apps: $installWinarenaApps"
Write-Host ""

# Load the shared setup-tools module
Import-Module (Join-Path $scriptFolder -ChildPath "setup-tools.psm1")

# Create a shortcut to the shared folder if it doesn't exist
if (-not (Test-Path $shortcutShared)) {
    New-Item -ItemType SymbolicLink -Path $shortcutShared -Value $scriptFolder
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

# Section - Disable Windows Backup and OneDrive prompts
Write-Host "Disabling Windows Backup and OneDrive prompts..."

# Disable Windows Backup via Registry
$backupRegPath = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\Backup"
if (-not (Test-Path $backupRegPath)) {
    New-Item -Path $backupRegPath -Force | Out-Null
}
# Disable Client backup
$clientBackupPath = "$backupRegPath\Client"
if (-not (Test-Path $clientBackupPath)) {
    New-Item -Path $clientBackupPath -Force | Out-Null
}
Set-ItemProperty -Path $clientBackupPath -Name "DisableBackupUI" -Value 1 -Type DWord -Force

# Disable OneDrive Backup prompts via Registry
$oneDriveRegPath = "HKCU:\SOFTWARE\Microsoft\OneDrive"
if (-not (Test-Path $oneDriveRegPath)) {
    New-Item -Path $oneDriveRegPath -Force | Out-Null
}
Set-ItemProperty -Path $oneDriveRegPath -Name "DisableFileSyncNGSC" -Value 1 -Type DWord -Force

# Disable OneDrive at system level
$oneDrivePolicyPath = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\OneDrive"
if (-not (Test-Path $oneDrivePolicyPath)) {
    New-Item -Path $oneDrivePolicyPath -Force | Out-Null
}
Set-ItemProperty -Path $oneDrivePolicyPath -Name "DisableFileSyncNGSC" -Value 1 -Type DWord -Force
Set-ItemProperty -Path $oneDrivePolicyPath -Name "DisableFileSync" -Value 1 -Type DWord -Force

# Disable Windows Backup reminder notifications
$notificationPath = "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Notifications\Settings\Windows.SystemToast.BackupReminder"
if (-not (Test-Path $notificationPath)) {
    New-Item -Path $notificationPath -Force | Out-Null
}
Set-ItemProperty -Path $notificationPath -Name "Enabled" -Value 0 -Type DWord -Force

# Disable cloud content suggestions and tips
$cloudContentPath = "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\ContentDeliveryManager"
if (Test-Path $cloudContentPath) {
    Set-ItemProperty -Path $cloudContentPath -Name "SubscribedContent-338389Enabled" -Value 0 -Type DWord -Force -ErrorAction SilentlyContinue
    Set-ItemProperty -Path $cloudContentPath -Name "SubscribedContent-310093Enabled" -Value 0 -Type DWord -Force -ErrorAction SilentlyContinue
    Set-ItemProperty -Path $cloudContentPath -Name "SoftLandingEnabled" -Value 0 -Type DWord -Force -ErrorAction SilentlyContinue
}

# Stop and disable OneDrive startup
$oneDriveExe = "$env:LOCALAPPDATA\Microsoft\OneDrive\OneDrive.exe"
if (Test-Path $oneDriveExe) {
    Stop-Process -Name "OneDrive" -Force -ErrorAction SilentlyContinue
}
# Remove OneDrive from startup
$startupRegPath = "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run"
Remove-ItemProperty -Path $startupRegPath -Name "OneDrive" -ErrorAction SilentlyContinue

Write-Host "Windows Backup and OneDrive prompts disabled."

# Section - Tools Installation

# Set TLS version to 1.2 or higher
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13

# Load the tools config json listing mirrors and aliases used for installing tools
$toolsConfigJsonPath = Join-Path $scriptFolder -ChildPath "tools_config.json"
$toolsConfigJson = Get-Content -Path $toolsConfigJsonPath -Raw
$toolsList = Get-Tools -toolsConfigJson $toolsConfigJson

## - uv (Python package manager)
Write-Host "Installing uv (Python package manager)..."
$uvInstallerScript = "$env:TEMP\uv_installer.ps1"
Invoke-WebRequest -Uri "https://astral.sh/uv/install.ps1" -OutFile $uvInstallerScript
powershell -ExecutionPolicy Bypass -File $uvInstallerScript

# Add uv to PATH - must be done AFTER installer completes
$uvPath = "$env:USERPROFILE\.local\bin"

# Update PATH for current session (required immediately after uv install)
$env:PATH = "$uvPath;$env:PATH"

# Also persist to user PATH for future sessions
Add-ToEnvPath -NewPath $uvPath

# Verify uv is accessible
Write-Host "Verifying uv installation..."
$uvExePath = Join-Path $uvPath "uv.exe"
if (Test-Path $uvExePath) {
    Write-Host "uv installed successfully at: $uvExePath"
} else {
    Write-Host "WARNING: uv.exe not found at expected path: $uvExePath"
    Write-Host "Waiting for installation to complete..."
    Start-Sleep -Seconds 5
    if (Test-Path $uvExePath) {
        Write-Host "uv.exe now available."
    } else {
        Write-Error "uv installation may have failed. Check manually."
    }
}

## - Python 3.12 via uv
Write-Host "Installing Python 3.12 via uv..."
uv python install 3.12

# Get the Python executable path from uv
$pythonExecutablePath = (uv python find 3.12).Trim()
Write-Host "Python 3.12 installed at: $pythonExecutablePath"

# Set python alias
$pythonAlias = "python"
$setAliasExpression = "Set-Alias -Name $pythonAlias -Value `"$pythonExecutablePath`""
Add-Content -Path $PROFILE -Value $setAliasExpression
Invoke-Expression $setAliasExpression

## - Git
$gitToolName = "git"
$gitToolDetails = Get-ToolDetails -toolsList $toolsList -toolName $gitToolName

# Check for Git installation
try {
    git --version | Out-Null
    Write-Host "Git is already installed."
} catch {
    Write-Host "Git is not installed. Downloading and installing Git..."
    $gitInstallerFilePath = "$env:TEMP\git_installer.exe"
    $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $gitToolDetails.mirrors -outfile $gitInstallerFilePath
    if (-not $downloadResult) {
        Write-Host "Failed to download Git. Please try again later or install manually."
    } else {
        Start-Process -FilePath $gitInstallerFilePath -Args "/VERYSILENT /NORESTART /NOCANCEL /SP-" -Wait
        Add-ToEnvPath -NewPath "C:\Program Files\Git\bin"

        Write-Host "Git has been installed."
    }
}

# ============================================================================
# OPTIONAL: Windows Arena Benchmark Apps
# ============================================================================
if ($installWinarenaApps) {
    Write-Host ""
    Write-Host "Installing Windows Arena benchmark apps..."
    Write-Host ""

    # - 7zip
    $7ZipToolName = "7zip"
    $7ZipToolDetails = Get-ToolDetails -toolsList $toolsList -toolName $7ZipToolName
    Write-Host "$7ZipToolDetails"

    if (Get-Command 7z -ErrorAction SilentlyContinue) {
        Write-Host "7-Zip is already installed."
    }
    else {
        Write-Host "Installing 7-Zip..."

        $7ZipInstallerFilePath = "$env:TEMP\7_zip.exe"
        Write-Host "$($7ZipToolDetails.mirrors)"
        $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $7ZipToolDetails.mirrors -outfile $7ZipInstallerFilePath
        if (-not $downloadResult) {
            Write-Host "Failed to download 7-Zip. Please try again later or install manually."
        } else {
            Start-Process -FilePath $7ZipInstallerFilePath -Args "/S" -Verb RunAs -Wait
            Remove-Item $7ZipInstallerFilePath

            # add 7z to PATH
            Add-ToEnvPath -NewPath "${env:ProgramFiles}\7-Zip"
        }
    }

# - ffpmeg
$ffpmegToolName = "ffmpeg"
$ffpmegToolDetails = Get-ToolDetails -toolsList $toolsList -toolName $ffpmegToolName

if (Get-Command ffmpeg -ErrorAction SilentlyContinue) {
    Write-Host "ffmpeg is already installed."
} else {
    Write-Host "ffmpeg is not installed. Installing it."
    $ffpmegInstallerFilePath = "C:\ffmpeg.7z"
    $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $ffpmegToolDetails.mirrors -outfile $ffpmegInstallerFilePath
    if (-not $downloadResult) {
        Write-Host "Failed to download ffmpeg. Please try again later or install manually."
    } else {
        Write-Host "Extracting $ffpmegInstallerFilePath..."
        7z x -y -o"C:\" "C:\ffmpeg.7z"

        $ffmpegFolder = Get-ChildItem -Path "C:\" -Filter "ffmpeg-*" -Directory
        $ffmpegFolder = -join ("C:\",  $ffmpegFolder)
        #remove ffmpeg folder if exists
        if (Test-Path "C:\ffmpeg") {
            Remove-Item -Path "C:\ffmpeg" -Recurse -Force
        }
        Rename-Item -Path "$ffmpegFolder" -NewName "ffmpeg"

        Write-Host "Adding ffmpeg to PATH..."
        Add-ToEnvPath -NewPath "C:\ffmpeg\bin"

        Write-Host "ffmpeg is installed"
    }
}

# Disable Edge Auto Updates
Stop-Process -Name "MicrosoftEdgeUpdate" -Force -ErrorAction SilentlyContinue
$edgeUpdatePath = "${env:ProgramFiles(x86)}\Microsoft\EdgeUpdate"
Remove-Item -Path $edgeUpdatePath -Recurse -Force -ErrorAction SilentlyContinue
Write-Host "Edge Update processes terminated and directory removed."

# - Google Chrome
$chromeToolName = "Google Chrome"
$chromeToolDetails = Get-ToolDetails -toolsList $toolsList -toolName $chromeToolName
$chromeExePath = "C:\Program Files\Google\Chrome\Application\chrome.exe"
$chromeAlias = $chromeToolDetails.alias

# Check if Google Chrome is already installed by its alias
if (Get-Command $chromeAlias -ErrorAction SilentlyContinue) {
    Write-Host "Google Chrome is already installed."
} else {
    # Download the installer to the Temp directory
    $chromeInstallerFilePath = "$env:TEMP\chrome_installer.exe"
    $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $chromeToolDetails.mirrors -outfile $chromeInstallerFilePath
    if (-not $downloadResult) {
        Write-Host "Failed to download Google Chrome. Please try again later or install manually."
    } else {
        # Execute the installer silently with elevated permissions
        Start-Process -FilePath $chromeInstallerFilePath -ArgumentList "/silent", "/install" -Verb RunAs -Wait

        # Remove the installer file after installation
        Remove-Item -Path $chromeInstallerFilePath

        # Set alias
        $setAliasExpression = "Set-Alias -Name $chromeAlias -Value `"$chromeExePath`""
        Add-Content -Path $PROFILE -Value $setAliasExpression
        Invoke-Expression $setAliasExpression

        # Add Chrome to the system PATH environment variable
        Add-ToEnvPath -NewPath "${env:ProgramFiles}\Google\Chrome\Application"

        # Disable Google Chrome Auto Updates
        $chromeRegPath = "HKLM:\SOFTWARE\Policies\Google\Update"
        if (-not (Test-Path $chromeRegPath)) {
            New-Item -Path $chromeRegPath -Force
        }
        Set-ItemProperty -Path $chromeRegPath -Name "AutoUpdateCheckPeriodMinutes" -Value 0
        Set-ItemProperty -Path $chromeRegPath -Name "UpdateDefault" -Value 0
    }
}

# - LibreOffice
$libreOfficeToolName = "LibreOffice"
$libreOfficeToolDetails = Get-ToolDetails -toolsList $toolsList -toolName $libreOfficeToolName

# Check for LibreOffice installation
$installedVersion = (Get-WmiObject -Query "SELECT * FROM Win32_Product WHERE Name like 'LibreOffice%'").Version
if (-not [string]::IsNullOrWhiteSpace($installedVersion)) {
    Write-Host "LibreOffice $version is already installed."
} else {
    Write-Host "LibreOffice is not installed. Downloading and installing LibreOffice..."
    $libreOfficeInstallerFilePath = "$env:TEMP\libreOffice_installer.exe"
    
    $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $libreOfficeToolDetails.mirrors -outfile $libreOfficeInstallerFilePath
    if (-not $downloadResult) {
        Write-Host "Failed to download LibreOffice. Please try again later or install manually."
    } else {
        Start-Process "msiexec.exe" -ArgumentList "/i `"$libreOfficeInstallerFilePath`" /quiet" -Wait -NoNewWindow
        Write-Host "LibreOffice has been installed."
    
        # Add LibreOffice to the system PATH environment variable
        Add-ToEnvPath -NewPath "C:\Program Files\LibreOffice\program"
    }
}

# - VLC
$vlcToolName = "VLC"
$vlcToolDetails = Get-ToolDetails -toolsList $toolsList -toolName $vlcToolName
$vlcAlias = $vlcToolDetails.alias
$vlcExecutableFilePath = "C:\Program Files\VideoLAN\VLC\vlc.exe"

# Check if VLC is already installed by checking the VLC command
if (Test-Path $vlcExecutableFilePath) {
    Write-Host "VLC is already installed."
} else {
    # Download the installer to the Temp directory
    $vlcInstallerFilePath = "$env:TEMP\vlc_installer.exe"
    $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $vlcToolDetails.mirrors -outfile $vlcInstallerFilePath
    if (-not $downloadResult) {
        Write-Host "Failed to download VLC. Please try again later or install manually."
    } else {
        # Execute the installer silently with elevated permissions
        Start-Process -FilePath $vlcInstallerFilePath -ArgumentList "/S" -Verb RunAs -Wait

        # Remove the installer file after installation
        Remove-Item -Path $vlcInstallerFilePath

        # Set alias
        $setAliasExpression = "Set-Alias -Name $vlcAlias -Value `"$vlcExecutableFilePath`""
        Add-Content -Path $PROFILE -Value $setAliasExpression
        Invoke-Expression $setAliasExpression

        # Add VLC to the system PATH environment variable
        Add-ToEnvPath -NewPath "C:\Program Files\VideoLAN\VLC"
    }
}

# - GIMP
$gimpToolName = "GIMP"
$gimpToolDetails = Get-ToolDetails -toolsList $toolsList -toolName $gimpToolName
$gimpAlias = $gimpToolDetails.alias
$gimpExecutablePath = "C:\Program Files\GIMP 2\bin\gimp-2.10.exe"

# Check if GIMP is already installed by checking the GIMP executable path
if (Test-Path $gimpExecutablePath) {
    Write-Host "GIMP is already installed."
} else {
    # Download the installer to the Temp directory
    $gimpInstallerFilePath = "$env:TEMP\gimp_installer.exe"
    $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $gimpToolDetails.mirrors -outfile $gimpInstallerFilePath
    if (-not $downloadResult) {
        Write-Host "Failed to download GIMP. Please try again later or install manually."
    } else {
        # Execute the installer silently with elevated permissions
        Start-Process -FilePath $gimpInstallerFilePath -ArgumentList "/VERYSILENT /ALLUSERS" -Verb RunAs -Wait

        # Remove the installer file after installation
        Remove-Item -Path $gimpInstallerFilePath

        # Set alias
        $setAliasExpression = "Set-Alias -Name $gimpAlias -Value `"$gimpExecutablePath`""
        Add-Content -Path $PROFILE -Value $setAliasExpression
        Invoke-Expression $setAliasExpression

        # Add GIMP to the system PATH environment variable
        Add-ToEnvPath -NewPath "C:\Program Files\GIMP 2\bin"
    }
}

# - VS Code
$vsCodeToolName = "VS Code"
$vsCodeToolDetails = Get-ToolDetails -toolsList $toolsList -toolName $vsCodeToolName
$vsCodeAlias = $gimpToolDetails.alias
$vsCodeExecutablePath = "C:\Users\$env:USERNAME\AppData\Local\Programs\Microsoft VS Code\Code.exe"

# Check if VS Code is already installed by checking the VS Code executable path
if (Test-Path $vsCodeExecutablePath) {
    Write-Host "VS Code is already installed."
} else {
    # Download the installer to the Temp directory
    $vsCodeInstallerFilePath = "$env:TEMP\VSCodeSetup.exe"
    $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $vsCodeToolDetails.mirrors -outfile $vsCodeInstallerFilePath
    if (-not $downloadResult) {
        Write-Host "Failed to download VS Code. Please try again later or install manually."
    } else {
        # Execute the installer silently with elevated permissions
        Start-Process -FilePath $vsCodeInstallerFilePath -ArgumentList "/VERYSILENT", "/mergetasks=!runcode" -Verb RunAs -Wait

        # Remove the installer file after installation
        Remove-Item -Path $vsCodeInstallerFilePath

        # Set alias
        $setAliasExpression = "Set-Alias -Name $vsCodeAlias -Value `"$vsCodeExecutablePath`""
        Add-Content -Path $PROFILE -Value $setAliasExpression
        Invoke-Expression $setAliasExpression

        # Add VS Code to the system PATH environment variable
        Add-ToEnvPath -NewPath "C:\Users\$env:USERNAME\AppData\Local\Programs\Microsoft VS Code\bin"

        # Disable Visual Studio Code Auto Updates
        $vsCodeSettingsPath = "${env:APPDATA}\Code\User\settings.json"
        if (-not (Test-Path $vsCodeSettingsPath)) {
            # Create the directory if it doesn't exist
            $dirPath = Split-Path -Path $vsCodeSettingsPath -Parent
            if (-not (Test-Path $dirPath)) {
                New-Item -ItemType Directory -Path $dirPath -Force
            }
            # Initialize an empty hashtable to act as the JSON object
            $settingsObj = @{}
            $settingsObj["update.mode"] = "none"  # Set update mode to none
            $settingsObj | ConvertTo-Json | Set-Content $vsCodeSettingsPath
        } else {
            # If the file exists, modify it
            $settingsObj = Get-Content $vsCodeSettingsPath | ConvertFrom-Json
            $settingsObj["update.mode"] = "none"
            $settingsObj | ConvertTo-Json | Set-Content $vsCodeSettingsPath
        }
    }
}

# - Thunderbird
$thunderbirdToolName = "Thunderbird"
$thunderbirdToolDetails = Get-ToolDetails -toolsList $toolsList -toolName $thunderbirdToolName
$thunderbirdAlias = $thunderbirdToolDetails.alias
$thunderbirdExecutablePath = "C:\Program Files\Mozilla Thunderbird\thunderbird.exe"

# Check if Thunderbird is already installed by checking the Thunderbird executable path
if (Test-Path $thunderbirdExecutablePath) {
    Write-Host "Thunderbird is already installed."
} else {
    # Download the installer to the Temp directory
    $thunderbirdInstallerFilePath = "$env:TEMP\ThunderbirdSetup.exe"
    $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $thunderbirdToolDetails.mirrors -outfile $thunderbirdInstallerFilePath
    if (-not $downloadResult) {
        Write-Host "Failed to download Thunderbird. Please try again later or install manually."
    } else {
        # Execute the installer silently with elevated permissions
        Start-Process -FilePath $thunderbirdInstallerFilePath -ArgumentList "/S" -Verb RunAs -Wait

        # Remove the installer file after installation
        Remove-Item -Path $thunderbirdInstallerFilePath

        # Set alias
        $setAliasExpression = "Set-Alias -Name $thunderbirdAlias -Value `"$thunderbirdExecutablePath`""
        Add-Content -Path $PROFILE -Value $setAliasExpression
        Invoke-Expression $setAliasExpression

        # Add Thunderbird to the system PATH environment variable
        Add-ToEnvPath -NewPath "C:\Program Files\Mozilla Thunderbird"
    }
}

    Write-Host ""
    Write-Host "Windows Arena apps installation complete."
} else {
    Write-Host "Skipping Windows Arena apps (minimal install)"
}

# ============================================================================
# CORE: Caddy Proxy (always installed - needed for cua-computer-server)
# ============================================================================

# - Caddy Proxy
$caddyProxyToolName = "Caddy Proxy"
$caddyProxyToolDetails = Get-ToolDetails -toolsList $toolsList -toolName $caddyProxyToolName
$caddyProxyAlias = $caddyProxyToolDetails.alias
$caddyProxyExecutablePath = "C:\Users\$env:USERNAME\caddy_windows_amd64.exe"

# Check if Caddy is already installed by checking the Caddy executable path
if (Test-Path $caddyProxyExecutablePath) {
    Write-Host "Caddy Server is already installed."
} else {
    # Download the installer to the Temp directory
    $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $caddyProxyToolDetails.mirrors -outfile $caddyProxyExecutablePath
    if (-not $downloadResult) {
        Write-Host "Failed to download Caddy Proxy. Please try again later or install manually."
    } else {
        # Set alias
        $setAliasExpression = "Set-Alias -Name $caddyProxyAlias -Value `"$caddyProxyExecutablePath`""
        Add-Content -Path $PROFILE -Value $setAliasExpression
        Invoke-Expression $setAliasExpression
    }
}

# - CUA Computer Server Setup

$cuaServerPort = 5000
$onLogonTaskName = "WindowsArena_OnLogon"

# Install cua-computer-server using uv
Write-Host "Installing cua-computer-server..."
uv tool install cua-computer-server

# Get uv tools bin path and add to PATH
$uvToolsBinPath = "$env:USERPROFILE\.local\bin"
Add-ToEnvPath -NewPath $uvToolsBinPath

# Add a firewall rule to allow incoming connections on the CUA server port
$cuaServerRuleName = "CUAComputerServer-$cuaServerPort"
if (-not (Get-NetFirewallRule -Name $cuaServerRuleName -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $cuaServerRuleName -Direction Inbound -Protocol TCP -LocalPort $cuaServerPort -Action Allow -Profile Any
    Write-Host "Firewall rule added to allow traffic on port $cuaServerPort for CUA Computer Server"
} else {
    Write-Host "Firewall rule already exists. $cuaServerRuleName"
}

# Add a firewall rule to allow incoming connections on the specified port for the Python executable
$caddyProxyRuleName = "Allow-Caddy-Proxy"
if (-not (Get-NetFirewallRule -Name $caddyProxyRuleName -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $caddyProxyRuleName -Direction Inbound -Program $caddyProxyExecutablePath -Action Allow -Profile Any
    Write-Host "Firewall rule added to allow traffic on port $caddyProxyRuleName"
} else {
    Write-Host "Firewall rule already exists. $caddyProxyRuleName "
}

$onLogonScriptPath = "$scriptFolder\on-logon.ps1"
# Check if the scheduled task exists before unregistering it
if (Get-ScheduledTask -TaskName $onLogonTaskName -ErrorAction SilentlyContinue) {
    Write-Host "Scheduled task $onLogonTaskName already exists."
} else {
    Write-Host "Registering new task $onLogonTaskName..."
    Register-LogonTask -TaskName $onLogonTaskName -ScriptPath $onLogonScriptPath -LocalUser "Docker"
}

Start-Sleep -Seconds 10
Start-ScheduledTask -TaskName $onLogonTaskName