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
Write-Host "Cua Windows Setup"
Write-Host "============================================"
Write-Host "Install Windows Arena Apps: $installWinarenaApps"
Write-Host ""

# =============================================================================
# BLANK DESKTOP: Suppress startup windows so instances launch to a clean desktop
# =============================================================================
Write-Host "Applying blank desktop hardening..."

# This script runs as SYSTEM during image provisioning, so HKCU writes would
# land in the SYSTEM hive rather than the Docker user's profile.  Instead, we
# load the Default User hive (C:\Users\Default\NTUSER.DAT) under a temporary
# HKLM mount and write all per-user registry tweaks there.  Any new user
# account created on this image (including "Docker") inherits the Default User
# hive on first logon, so the values will be present from the very first login.
$defaultUserHivePath = 'C:\Users\Default\NTUSER.DAT'
$tempHiveMount        = 'HKLM:\TempDefaultUser'
$tempHiveMountReg     = 'HKLM\TempDefaultUser'   # reg.exe / REG LOAD syntax (no colon)

Write-Host "  Loading Default User hive from $defaultUserHivePath..."
try {
    # Unload any stale mount from a previous failed run before loading
    if (Test-Path $tempHiveMount) {
        & reg.exe UNLOAD $tempHiveMountReg 2>&1 | Out-Null
    }
    $regLoadResult = & reg.exe LOAD $tempHiveMountReg $defaultUserHivePath 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "reg.exe LOAD failed (exit $LASTEXITCODE): $regLoadResult"
    }
    Write-Host "  Default User hive loaded at $tempHiveMount"
} catch {
    Write-Host "  WARNING: Could not load Default User hive — per-user tweaks skipped: $($_.Exception.Message)"
    # Skip the per-user block entirely; HKLM policy tweaks below still apply.
    $tempHiveMount = $null
}

if ($tempHiveMount) {
    try {
        # ------------------------------------------------------------------
        # Explorer: disable 'restore previous folder windows on logon'
        # ------------------------------------------------------------------
        try {
            New-Item -Path "$tempHiveMount\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" -Force -ErrorAction Stop | Out-Null
            Set-ItemProperty -Path "$tempHiveMount\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" -Name 'PersistBrowsers' -Value 0 -Type DWord -ErrorAction Stop
            Write-Host "  Explorer: PersistBrowsers disabled"
        } catch { Write-Host "  Explorer PersistBrowsers warning: $($_.Exception.Message)" }

        # ------------------------------------------------------------------
        # Explorer: disable 'show recently used files/folders' in Quick Access
        # ------------------------------------------------------------------
        try {
            Set-ItemProperty -Path "$tempHiveMount\Software\Microsoft\Windows\CurrentVersion\Explorer" -Name 'ShowRecent'   -Value 0 -Type DWord -ErrorAction Stop
            Set-ItemProperty -Path "$tempHiveMount\Software\Microsoft\Windows\CurrentVersion\Explorer" -Name 'ShowFrequent' -Value 0 -Type DWord -ErrorAction Stop
            Write-Host "  Explorer: recent/frequent items disabled"
        } catch { Write-Host "  Explorer recent items warning: $($_.Exception.Message)" }

        # ------------------------------------------------------------------
        # Taskbar: hide Widgets (News and Interests) and Chat icons
        # ------------------------------------------------------------------
        try {
            New-Item -Path "$tempHiveMount\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\Advanced" -Force -ErrorAction Stop | Out-Null
            Set-ItemProperty -Path "$tempHiveMount\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\Advanced" -Name 'TaskbarDa' -Value 0 -Type DWord -ErrorAction Stop
            Set-ItemProperty -Path "$tempHiveMount\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\Advanced" -Name 'TaskbarMn' -Value 0 -Type DWord -ErrorAction Stop
            Write-Host "  Taskbar: Widgets and Chat hidden"
        } catch { Write-Host "  Taskbar widgets warning: $($_.Exception.Message)" }

        # ------------------------------------------------------------------
        # ContentDeliveryManager: suppress "New features and suggestions"
        # ------------------------------------------------------------------
        try {
            $cdm = "$tempHiveMount\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager"
            New-Item -Path $cdm -Force -ErrorAction Stop | Out-Null
            Set-ItemProperty -Path $cdm -Name 'SubscribedContent-338393Enabled' -Value 0 -Type DWord -ErrorAction Stop
            Set-ItemProperty -Path $cdm -Name 'SubscribedContent-353696Enabled' -Value 0 -Type DWord -ErrorAction Stop
            Set-ItemProperty -Path $cdm -Name 'SubscribedContent-353694Enabled' -Value 0 -Type DWord -ErrorAction Stop
            Set-ItemProperty -Path $cdm -Name 'SoftLandingEnabled'              -Value 0 -Type DWord -ErrorAction Stop
            Set-ItemProperty -Path $cdm -Name 'FeatureManagementEnabled'        -Value 0 -Type DWord -ErrorAction Stop
            Write-Host "  ContentDeliveryManager: suggestions disabled"
        } catch { Write-Host "  ContentDeliveryManager warning: $($_.Exception.Message)" }

        # ------------------------------------------------------------------
        # Startup cleanup: remove per-user startup entries (HKCU Run key) that
        # can open windows on login (OneDrive, Teams, SecurityHealth, etc.)
        # The HKLM Run key is handled separately below after hive unload.
        # Fix: use -ErrorAction Stop so real failures surface; catch handles
        # the expected "property does not exist" case without noisy output.
        # ------------------------------------------------------------------
        $startupEntriesToRemove = @('OneDrive', 'MicrosoftTeams', 'Teams', 'SecurityHealth')
        foreach ($entry in $startupEntriesToRemove) {
            try {
                Remove-ItemProperty -Path "$tempHiveMount\Software\Microsoft\Windows\CurrentVersion\Run" -Name $entry -ErrorAction Stop
            } catch [System.Management.Automation.PSArgumentException] {
                # Property does not exist — expected, not an error
            } catch {
                Write-Host "  Startup cleanup warning: DefaultUser Run key, entry='$entry': $($_.Exception.Message)"
            }
        }
        Write-Host "  Startup: per-user startup entries removed from Default User hive"

    } finally {
        # Always unload the hive to avoid handle leaks
        Write-Host "  Unloading Default User hive..."
        [gc]::Collect()
        [gc]::WaitForPendingFinalizers()
        $regUnloadResult = & reg.exe UNLOAD $tempHiveMountReg 2>&1
        if ($LASTEXITCODE -ne 0) {
            Write-Host "  WARNING: Failed to unload Default User hive (exit $LASTEXITCODE): $regUnloadResult"
        } else {
            Write-Host "  Default User hive unloaded."
        }
    }
}

# Remove common startup entries from the machine-wide HKLM Run key
# (OneDrive, Microsoft Teams, Windows Security, etc.)
# Fix: use -ErrorAction Stop so real failures surface in the catch block;
# the catch distinguishes missing-property (expected) from actual errors.
$startupEntriesToRemove = @('OneDrive', 'MicrosoftTeams', 'Teams', 'SecurityHealth')
foreach ($entry in $startupEntriesToRemove) {
    try {
        Remove-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run' -Name $entry -ErrorAction Stop
    } catch [System.Management.Automation.PSArgumentException] {
        # Property does not exist — expected, not an error
    } catch {
        Write-Host "  Startup cleanup warning: HKLM Run key, entry='$entry': $($_.Exception.Message)"
    }
}
Write-Host "  Startup: HKLM startup entries removed"

# Disable the 'Start' menu recommended apps and news on first login
# (HKLM policy — machine-wide, no per-user hive needed)
try {
    $startPolicies = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\Explorer'
    New-Item -Path $startPolicies -Force -ErrorAction Stop | Out-Null
    Set-ItemProperty -Path $startPolicies -Name 'HideRecentlyAddedApps' -Value 1 -Type DWord -ErrorAction Stop
    Write-Host "  Start menu: recently added apps hidden"
} catch { Write-Host "  Start menu warning: $($_.Exception.Message)" }

Write-Host "Blank desktop hardening complete."
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

# Cua Computer Server Setup
Write-Host "Setting up Cua Computer Server..."
$cuaServerSetupScript = Join-Path $scriptFolder -ChildPath "setup-cua-server.ps1"
if (Test-Path $cuaServerSetupScript) {
    & $cuaServerSetupScript
    Write-Host "Cua Computer Server setup completed."
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
Write-Host "Cua Windows Setup Complete!"
Write-Host "============================================"