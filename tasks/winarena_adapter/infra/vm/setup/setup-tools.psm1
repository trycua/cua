function Get-Tools {
    param(
        [string]$toolsConfigJson
    )

    # Convert the JSON string to a PowerShell object
    $toolsList = $toolsConfigJson | ConvertFrom-Json

    return $toolsList
}

function Get-ToolDetails {
    param(
        $toolsList,
        [string]$toolName
    )

    # Check if the program exists in the JSON data
    if ($toolsList.PSObject.Properties.Name -contains $toolName) {
        # Return the program details as a PowerShell object
        return $toolsList.$toolName
    } else {
        # Handle the case where the program is not found
        Write-Host "Program '$toolName' not found in the list."
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
            Write-Host "Error downloading from $url. Please check and update the mirrors."
        }
    }

    Write-Host "Downloading from the provided mirrors failed. Please check and update the mirrors."
    return $false
}

function Invoke-DownloadFile {
    param (
        [string]$url,
        [string]$outfile
    )
    # Makes download faster by disabling progress bar
    $ProgressPreference = "SilentlyContinue"

    $retryCount = 0
    $maxRetries = 3
    $sleepSeconds = 2
    $maxSleepSeconds = 10
    $userAgent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/58.0.3029.110 Safari/537.36"

    # Ensure directory exists
    $directory = Split-Path -Path $outfile -Parent
    if (-Not (Test-Path -Path $directory)) {
        Write-Host "Creating directory $directory..."
        New-Item -Path $directory -ItemType Directory -Force | Out-Null
    }

    while ($retryCount -lt $maxRetries) {
        try {
            Invoke-RestMethod -Uri $url -OutFile $outfile -Headers @{"User-Agent" = $userAgent}
            Write-Host "Download successful, file saved to: $outfile"
            break
        } catch {
            $retryCount++
            Write-Host "Attempt $retryCount of $maxRetries failed. Error: $($_.Exception.Message)"
            Start-Sleep -Seconds $sleepSeconds
            $sleepSeconds = [Math]::Min($sleepSeconds * 2, $maxSleepSeconds)  # Exponential backoff with a cap
        }
    }

    if ($retryCount -eq $maxRetries) {
        Write-Host "Failed to download the file after $maxRetries attempts."
        return $false
    }

    return $true
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

# Function to attempt pip install and handle failures
function Install-PythonPackages {
    param (
        [string]$Package = "",
        [string]$Arguments = "",
        [string]$RequirementsPath = ""
    )
    $RetryCount = 3
    $currentAttempt = 0
    while ($currentAttempt -lt $RetryCount) {
        if (-not [string]::IsNullOrWhiteSpace($RequirementsPath)) {
            & python -m pip install --no-cache-dir -r $RequirementsPath $Arguments
        } else {
            & python -m pip install --no-cache-dir $Package $Arguments
        }
        if ($LASTEXITCODE -eq 0) {
            Write-Host "Installation successful."
            return
        } else {
            Write-Host "Attempt $($currentAttempt + 1) failed. Retrying..."
            Start-Sleep -Seconds 10
            $currentAttempt++
        }
    }
    Write-Error "Failed to install after $RetryCount attempts."
    exit
}