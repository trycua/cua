$ErrorActionPreference = "Continue"

$scriptFolder = "C:\OEM"

Import-Module (Join-Path $scriptFolder -ChildPath "setup-utils.psm1")

Set-StrictMode -Version Latest

# Set TLS version to 1.2 or higher
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13

# =============================================================================
# Check if we should install Windows Arena benchmark apps
# Set INSTALL_WINARENA_APPS=true to install Chrome, LibreOffice, VLC, etc.
# =============================================================================
$installWinarenaApps = $false
$configFilePath = Join-Path $scriptFolder -ChildPath "install_config.json"
if ($env:INSTALL_WINARENA_APPS -eq "true") {
    $installWinarenaApps = $true
} elseif (Test-Path $configFilePath) {
    try {
        $config = Get-Content -Path $configFilePath -Raw | ConvertFrom-Json
        if ($config.INSTALL_WINARENA_APPS -eq $true) {
            $installWinarenaApps = $true
        }
    } catch {
        Write-Host "Warning: Could not read install_config.json"
    }
}

Write-Host "============================================"
Write-Host "CUA Windows Setup"
Write-Host "============================================"
Write-Host "Install Windows Arena Apps: $installWinarenaApps"
Write-Host ""

# =============================================================================
# CORE: Git (always installed)
# =============================================================================

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

# =============================================================================
# OPTIONAL: Windows Arena Benchmark Apps
# =============================================================================
if ($installWinarenaApps) {
    Write-Host ""
    Write-Host "============================================"
    Write-Host "Installing Windows Arena benchmark apps..."
    Write-Host "============================================"

    # Load tools config
    $toolsConfigJsonPath = Join-Path $scriptFolder -ChildPath "tools_config.json"
    if (-not (Test-Path $toolsConfigJsonPath)) {
        Write-Host "Warning: tools_config.json not found, skipping app installation"
    } else {
        $toolsConfigJson = Get-Content -Path $toolsConfigJsonPath -Raw
        $toolsList = Get-Tools -toolsConfigJson $toolsConfigJson

        # --- 7zip ---
        Write-Host "Installing 7-Zip..."
        $7ZipToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "7zip"
        if ($7ZipToolDetails -and -not (Get-Command 7z -ErrorAction SilentlyContinue)) {
            $7ZipInstallerFilePath = "$env:TEMP\7_zip.exe"
            $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $7ZipToolDetails.mirrors -outfile $7ZipInstallerFilePath
            if ($downloadResult) {
                Start-Process -FilePath $7ZipInstallerFilePath -Args "/S" -Verb RunAs -Wait
                Remove-Item $7ZipInstallerFilePath -ErrorAction SilentlyContinue
                Add-ToEnvPath -NewPath "${env:ProgramFiles}\7-Zip"
                Write-Host "[OK] 7-Zip installed"
            }
        } else {
            Write-Host "[OK] 7-Zip already installed"
        }

        # --- ffmpeg ---
        Write-Host "Installing ffmpeg..."
        $ffmpegToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "ffmpeg"
        if ($ffmpegToolDetails -and -not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) {
            $ffmpegInstallerFilePath = "C:\ffmpeg.7z"
            $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $ffmpegToolDetails.mirrors -outfile $ffmpegInstallerFilePath
            if ($downloadResult) {
                & 7z x -y -o"C:\" "C:\ffmpeg.7z" | Out-Null
                $ffmpegFolder = Get-ChildItem -Path "C:\" -Filter "ffmpeg-*" -Directory | Select-Object -First 1
                if ($ffmpegFolder) {
                    if (Test-Path "C:\ffmpeg") { Remove-Item -Path "C:\ffmpeg" -Recurse -Force }
                    Rename-Item -Path $ffmpegFolder.FullName -NewName "ffmpeg"
                    Add-ToEnvPath -NewPath "C:\ffmpeg\bin"
                }
                Remove-Item $ffmpegInstallerFilePath -ErrorAction SilentlyContinue
                Write-Host "[OK] ffmpeg installed"
            }
        } else {
            Write-Host "[OK] ffmpeg already installed"
        }

        # --- Google Chrome ---
        Write-Host "Installing Google Chrome..."
        $chromeToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "Google Chrome"
        $chromeExePath = "C:\Program Files\Google\Chrome\Application\chrome.exe"
        if ($chromeToolDetails -and -not (Test-Path $chromeExePath)) {
            $chromeInstallerFilePath = "$env:TEMP\chrome_installer.exe"
            $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $chromeToolDetails.mirrors -outfile $chromeInstallerFilePath
            if ($downloadResult) {
                Start-Process -FilePath $chromeInstallerFilePath -ArgumentList "/silent", "/install" -Verb RunAs -Wait
                Remove-Item -Path $chromeInstallerFilePath -ErrorAction SilentlyContinue
                Add-ToEnvPath -NewPath "${env:ProgramFiles}\Google\Chrome\Application"
                # Disable auto updates
                $chromeRegPath = "HKLM:\SOFTWARE\Policies\Google\Update"
                if (-not (Test-Path $chromeRegPath)) { New-Item -Path $chromeRegPath -Force | Out-Null }
                Set-ItemProperty -Path $chromeRegPath -Name "AutoUpdateCheckPeriodMinutes" -Value 0
                Set-ItemProperty -Path $chromeRegPath -Name "UpdateDefault" -Value 0
                Write-Host "[OK] Chrome installed"
            }
        } else {
            Write-Host "[OK] Chrome already installed"
        }

        # --- LibreOffice ---
        Write-Host "Installing LibreOffice..."
        $libreOfficeToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "LibreOffice"
        $installedVersion = (Get-WmiObject -Query "SELECT * FROM Win32_Product WHERE Name like 'LibreOffice%'" -ErrorAction SilentlyContinue).Version
        if ($libreOfficeToolDetails -and [string]::IsNullOrWhiteSpace($installedVersion)) {
            $libreOfficeInstallerFilePath = "$env:TEMP\libreOffice_installer.msi"
            $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $libreOfficeToolDetails.mirrors -outfile $libreOfficeInstallerFilePath
            if ($downloadResult) {
                Start-Process "msiexec.exe" -ArgumentList "/i `"$libreOfficeInstallerFilePath`" /quiet" -Wait -NoNewWindow
                Add-ToEnvPath -NewPath "C:\Program Files\LibreOffice\program"
                Write-Host "[OK] LibreOffice installed"
            }
        } else {
            Write-Host "[OK] LibreOffice already installed"
        }

        # --- VLC ---
        Write-Host "Installing VLC..."
        $vlcToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "VLC"
        $vlcExecutableFilePath = "C:\Program Files\VideoLAN\VLC\vlc.exe"
        if ($vlcToolDetails -and -not (Test-Path $vlcExecutableFilePath)) {
            $vlcInstallerFilePath = "$env:TEMP\vlc_installer.exe"
            $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $vlcToolDetails.mirrors -outfile $vlcInstallerFilePath
            if ($downloadResult) {
                Start-Process -FilePath $vlcInstallerFilePath -ArgumentList "/S" -Verb RunAs -Wait
                Remove-Item -Path $vlcInstallerFilePath -ErrorAction SilentlyContinue
                Add-ToEnvPath -NewPath "C:\Program Files\VideoLAN\VLC"
                Write-Host "[OK] VLC installed"
            }
        } else {
            Write-Host "[OK] VLC already installed"
        }

        # --- GIMP ---
        Write-Host "Installing GIMP..."
        $gimpToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "GIMP"
        $gimpExecutablePath = "C:\Program Files\GIMP 2\bin\gimp-2.10.exe"
        if ($gimpToolDetails -and -not (Test-Path $gimpExecutablePath)) {
            $gimpInstallerFilePath = "$env:TEMP\gimp_installer.exe"
            $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $gimpToolDetails.mirrors -outfile $gimpInstallerFilePath
            if ($downloadResult) {
                Start-Process -FilePath $gimpInstallerFilePath -ArgumentList "/VERYSILENT /ALLUSERS" -Verb RunAs -Wait
                Remove-Item -Path $gimpInstallerFilePath -ErrorAction SilentlyContinue
                Add-ToEnvPath -NewPath "C:\Program Files\GIMP 2\bin"
                Write-Host "[OK] GIMP installed"
            }
        } else {
            Write-Host "[OK] GIMP already installed"
        }

        # --- VS Code ---
        Write-Host "Installing VS Code..."
        $vsCodeToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "VS Code"
        $vsCodeExecutablePath = "C:\Users\$env:USERNAME\AppData\Local\Programs\Microsoft VS Code\Code.exe"
        if ($vsCodeToolDetails -and -not (Test-Path $vsCodeExecutablePath)) {
            $vsCodeInstallerFilePath = "$env:TEMP\VSCodeSetup.exe"
            $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $vsCodeToolDetails.mirrors -outfile $vsCodeInstallerFilePath
            if ($downloadResult) {
                Start-Process -FilePath $vsCodeInstallerFilePath -ArgumentList "/VERYSILENT", "/mergetasks=!runcode" -Verb RunAs -Wait
                Remove-Item -Path $vsCodeInstallerFilePath -ErrorAction SilentlyContinue
                Add-ToEnvPath -NewPath "C:\Users\$env:USERNAME\AppData\Local\Programs\Microsoft VS Code\bin"
                Write-Host "[OK] VS Code installed"
            }
        } else {
            Write-Host "[OK] VS Code already installed"
        }

        # --- Thunderbird ---
        Write-Host "Installing Thunderbird..."
        $thunderbirdToolDetails = Get-ToolDetails -toolsList $toolsList -toolName "Thunderbird"
        $thunderbirdExecutablePath = "C:\Program Files\Mozilla Thunderbird\thunderbird.exe"
        if ($thunderbirdToolDetails -and -not (Test-Path $thunderbirdExecutablePath)) {
            $thunderbirdInstallerFilePath = "$env:TEMP\ThunderbirdSetup.exe"
            $downloadResult = Invoke-DownloadFileFromAvailableMirrors -mirrorUrls $thunderbirdToolDetails.mirrors -outfile $thunderbirdInstallerFilePath
            if ($downloadResult) {
                Start-Process -FilePath $thunderbirdInstallerFilePath -ArgumentList "/S" -Verb RunAs -Wait
                Remove-Item -Path $thunderbirdInstallerFilePath -ErrorAction SilentlyContinue
                Add-ToEnvPath -NewPath "C:\Program Files\Mozilla Thunderbird"
                Write-Host "[OK] Thunderbird installed"
            }
        } else {
            Write-Host "[OK] Thunderbird already installed"
        }

        Write-Host ""
        Write-Host "[OK] Windows Arena apps installation complete"
    }
} else {
    Write-Host "Skipping Windows Arena apps (minimal install)"
}

Write-Host ""
Write-Host "============================================"
Write-Host "CUA Windows Setup Complete!"
Write-Host "============================================"