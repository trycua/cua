$ErrorActionPreference = "Continue" # until downloading from mirrors is more stable

# Section - General Setup
$shortcutShared = "C:\Users\Docker\Desktop\Setup"
$scriptFolder = "\\host.lan\Data"
$toolsFolder = "C:\Users\$env:USERNAME\Tools"

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

# Section - Tools Installation

# Set TLS version to 1.2 or higher
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13

# Load the tools config json listing mirrors and aliases used for installing tools
$toolsConfigJsonPath = Join-Path $scriptFolder -ChildPath "tools_config.json"
$toolsConfigJson = Get-Content -Path $toolsConfigJsonPath -Raw
$toolsList = Get-Tools -toolsConfigJson $toolsConfigJson

## - Python
$pythonToolName = "Python"
$userPythonPath = "$env:LOCALAPPDATA\Programs\Python"
$pythonDetails = Get-ToolDetails -toolsList $toolsList -toolName $pythonToolName
$pythonAlias = $pythonDetails.alias

# Check for Python installation
$pythonExecutablePath = Get-ChildItem -Path $userPythonPath -Filter python.exe -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName

# Force to install Python 3.10 as the pre-installed version on Windows may not work sometimes
Write-Host "Downloading Python $pythonVersion..."
$pythonInstallerFilePath = "$env:TEMP\python_installer.exe"
$downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $pythonDetails.mirrors -outfile $pythonInstallerFilePath
if (-not $downloadResult) {
    Write-Host "Failed to download Python. Please try again later or install manually."
} else {
    Write-Host "Installing Python for current user..."
    Start-Process -FilePath $pythonInstallerFilePath -Args "/quiet InstallAllUsers=0 PrependPath=0" -NoNewWindow -Wait
    $pythonExecutablePath = "$userPythonPath\Python310\python.exe"
    $setAliasExpression = "Set-Alias -Name $pythonAlias -Value `"$pythonExecutablePath`""
    Add-Content -Path $PROFILE -Value $setAliasExpression
    Invoke-Expression $setAliasExpression
}

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

# - Caddy Reverse Proxy Setup
Write-Host "Setting up Caddy Reverse Proxy..."
$caddySetupScript = Join-Path $scriptFolder -ChildPath "setup-caddy-proxy.ps1"
if (Test-Path $caddySetupScript) {
    & $caddySetupScript
    Write-Host "Caddy Reverse Proxy setup completed."
} else {
    Write-Host "ERROR: setup-caddy-proxy.ps1 not found at $caddySetupScript"
}

# Add a firewall rule for Caddy Proxy if needed
$caddyProxyRuleName = "Allow-Caddy-Proxy"
if (-not (Get-NetFirewallRule -Name $caddyProxyRuleName -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $caddyProxyRuleName -Direction Inbound -Program $caddyProxyExecutablePath -Action Allow -Profile Any
    Write-Host "Firewall rule added to allow traffic for $caddyProxyRuleName"
} else {
    Write-Host "Firewall rule already exists. $caddyProxyRuleName"
}

# - CUA Computer Server Setup
Write-Host "Setting up CUA Computer Server..."
$cuaServerSetupScript = Join-Path $scriptFolder -ChildPath "setup-cua-server.ps1"
if (Test-Path $cuaServerSetupScript) {
    & $cuaServerSetupScript
    Write-Host "CUA Computer Server setup completed."
} else {
    Write-Host "ERROR: setup-cua-server.ps1 not found at $cuaServerSetupScript"
}

# Register on-logon task (required for base image shutdown detection)
$onLogonTaskName = "WindowsArena_OnLogon"
$onLogonScriptPath = "$scriptFolder\on-logon.ps1"
if (Get-ScheduledTask -TaskName $onLogonTaskName -ErrorAction SilentlyContinue) {
    Write-Host "Scheduled task $onLogonTaskName already exists."
} else {
    Write-Host "Registering new task $onLogonTaskName..."
    Register-LogonTask -TaskName $onLogonTaskName -ScriptPath $onLogonScriptPath -LocalUser "Docker"
}

Start-Sleep -Seconds 10

# Start the on-logon task asynchronously (non-blocking) so setup can complete
Write-Host "Starting $onLogonTaskName task in background..."
Start-Process -WindowStyle Hidden -FilePath "powershell.exe" -ArgumentList "-Command", "Start-ScheduledTask -TaskName '$onLogonTaskName'"
