function Write-Log {
    param(
        [Parameter(Mandatory=$true)]
        [string]$LogFile,

        [Parameter(Mandatory=$true)]
        [string]$Message
    )

    $timestamp = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
    "$timestamp`t$Message" | Tee-Object -FilePath $LogFile -Append
    Write-Host "$timestamp`t$Message"
}

function Resolve-ChocoPath {
    param(
        [switch]$SkipInstall
    )

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

    if (-not $SkipInstall) {
        Write-Host "Chocolatey not found. Installing..."
        Set-ExecutionPolicy Bypass -Scope Process -Force
        [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.SecurityProtocolType]::Tls12
        Invoke-Expression ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))

        return Resolve-ChocoPath -SkipInstall
    }

    return $null
}

function Add-ToEnvPath {
    param (
        [string]$NewPath
    )

    # Get the current PATH environment variable
    $envPath = [Environment]::GetEnvironmentVariable("PATH", "Machine")

    # Append the new path to the existing PATH
    $newPath = "$envPath;$NewPath"

    # Set the updated PATH environment variable
    [Environment]::SetEnvironmentVariable("PATH", $newPath, "Machine")

    # Fetch updates from the shell
    $env:PATH += ";${newPath}"
}

# =============================================================================
# Tools Config Helpers (for Windows Arena apps)
# =============================================================================

function Get-Tools {
    param(
        [string]$toolsConfigJson
    )
    $toolsList = $toolsConfigJson | ConvertFrom-Json
    return $toolsList
}

function Get-ToolDetails {
    param(
        $toolsList,
        [string]$toolName
    )
    if ($toolsList.PSObject.Properties.Name -contains $toolName) {
        return $toolsList.$toolName
    } else {
        Write-Host "Tool '$toolName' not found in config."
        return $null
    }
}

function Invoke-DownloadFileFromAvailableMirrors {
    param (
        [string[]]$mirrorUrls,
        [string]$outfile
    )
    foreach ($url in $mirrorUrls) {
        try {
            $result = Invoke-DownloadFile -url $url -outfile $outfile
            if ($result -eq $true) {
                Write-Host "Downloaded using $url"
                return $true
            }
        } catch {
            Write-Host "Error downloading from $url"
        }
    }
    Write-Host "All mirrors failed for $outfile"
    return $false
}

function Invoke-DownloadFile {
    param (
        [string]$url,
        [string]$outfile
    )
    $ProgressPreference = "SilentlyContinue"
    $retryCount = 0
    $maxRetries = 3
    $sleepSeconds = 2
    $userAgent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"

    $directory = Split-Path -Path $outfile -Parent
    if ($directory -and -Not (Test-Path -Path $directory)) {
        New-Item -Path $directory -ItemType Directory -Force | Out-Null
    }

    while ($retryCount -lt $maxRetries) {
        try {
            Invoke-RestMethod -Uri $url -OutFile $outfile -Headers @{"User-Agent" = $userAgent}
            Write-Host "Download successful: $outfile"
            return $true
        } catch {
            $retryCount++
            Write-Host "Attempt $retryCount of $maxRetries failed: $($_.Exception.Message)"
            Start-Sleep -Seconds $sleepSeconds
            $sleepSeconds = [Math]::Min($sleepSeconds * 2, 10)
        }
    }
    return $false
}

# =============================================================================
# Scheduled Task Helpers
# =============================================================================

function Register-LogonTask {
    param(

        [parameter(Mandatory = $true, ValueFromPipelineByPropertyName = $true, HelpMessage = "Name of the scheduled task")]
        [string]
        $TaskName,

        [parameter(Mandatory = $true, ValueFromPipelineByPropertyName = $true, HelpMessage = "Path to the .py script")]
        [string]
        $ScriptPath,

        [parameter(Mandatory = $false, ValueFromPipelineByPropertyName = $true, HelpMessage = "Arguments to the .py script")]
        [string]
        $Arguments = "",

        [parameter(Mandatory = $false, ValueFromPipelineByPropertyName = $true, HelpMessage = "Local Account username")]
        [string]
        $LocalUser,

        [parameter(Mandatory = $false, ValueFromPipelineByPropertyName = $true, HelpMessage = "Local Account password")]
        [string]
        $LocalPassword,

        [parameter(Mandatory = $false, ValueFromPipelineByPropertyName = $true, HelpMessage = "Whether to execute the command as SYSTEM")]
        [switch]
        $AsSystem = $false,

        [parameter(Mandatory = $false, ValueFromPipelineByPropertyName = $true, HelpMessage = "logging file")]
        [string]
        $LogFilePath
    )

    $scriptDirectory = Split-Path $ScriptPath

    $taskActionArgument = "-ExecutionPolicy Bypass -windowstyle hidden -Command `"try { . '$ScriptPath' $Arguments } catch { Write `$_.Exception.Message | Out-File $($TaskName)_Log.txt } finally { } `""
    $taskAction = New-ScheduledTaskAction -Execute "$PSHome\powershell.exe" -Argument $taskActionArgument -WorkingDirectory $scriptDirectory

    $params = @{
        Force    = $True
        Action   = $taskAction
        RunLevel = "Highest"
        TaskName = $TaskName
    }

    $taskTrigger = New-ScheduledTaskTrigger -AtLogOn
    $params.Add("Trigger", $taskTrigger)

    if ($AsSystem) {
        $params.Add("User", "NT AUTHORITY\SYSTEM")
    }
    else {
        $params.Add("User", $LocalUser)
        if ($LocalPassword) {
            $params.Add("Password", $LocalPassword)
        }
    }

    Write-Host "Registering scheduled task '$TaskName' to run 'powershell.exe $taskActionArgument'..."
    Register-ScheduledTask @params
}
