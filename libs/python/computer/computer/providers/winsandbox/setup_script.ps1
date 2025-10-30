# Setup script for Windows Sandbox CUA Computer provider
# This script runs when the sandbox starts

Write-Host "Starting CUA Computer setup in Windows Sandbox..."

# Function to find the mapped Python installation from pywinsandbox
function Find-MappedPython {
    Write-Host "Looking for mapped Python installation from pywinsandbox..."
    
    # pywinsandbox maps the host Python installation to the sandbox
    # Look for mapped shared folders on the desktop (common pywinsandbox pattern)
    $desktopPath = "C:\Users\WDAGUtilityAccount\Desktop"
    $sharedFolders = Get-ChildItem -Path $desktopPath -Directory -ErrorAction SilentlyContinue
    
    foreach ($folder in $sharedFolders) {
        # Look for Python executables in shared folders
        $pythonPaths = @(
            "$($folder.FullName)\python.exe",
            "$($folder.FullName)\Scripts\python.exe",
            "$($folder.FullName)\bin\python.exe"
        )
        
        foreach ($pythonPath in $pythonPaths) {
            if (Test-Path $pythonPath) {
                try {
                    $version = & $pythonPath --version 2>&1
                    if ($version -match "Python") {
                        Write-Host "Found mapped Python: $pythonPath - $version"
                        return $pythonPath
                    }
                } catch {
                    continue
                }
            }
        }
        
        # Also check subdirectories that might contain Python
        $subDirs = Get-ChildItem -Path $folder.FullName -Directory -ErrorAction SilentlyContinue
        foreach ($subDir in $subDirs) {
            $pythonPath = "$($subDir.FullName)\python.exe"
            if (Test-Path $pythonPath) {
                try {
                    $version = & $pythonPath --version 2>&1
                    if ($version -match "Python") {
                        Write-Host "Found mapped Python in subdirectory: $pythonPath - $version"
                        return $pythonPath
                    }
                } catch {
                    continue
                }
            }
        }
    }
    
    # Fallback: try common Python commands that might be available
    $pythonCommands = @("python", "py", "python3")
    foreach ($cmd in $pythonCommands) {
        try {
            $version = & $cmd --version 2>&1
            if ($version -match "Python") {
                Write-Host "Found Python via command '$cmd': $version"
                return $cmd
            }
        } catch {
            continue
        }
    }
    
    throw "Could not find any Python installation (mapped or otherwise)"
}

try {
    # Step 1: Find the mapped Python installation
    Write-Host "Step 1: Finding mapped Python installation..."
    $pythonExe = Find-MappedPython
    Write-Host "Using Python: $pythonExe"
    
    # Verify Python works and show version
    $pythonVersion = & $pythonExe --version 2>&1
    Write-Host "Python version: $pythonVersion"

    # Step 2: Create a dedicated virtual environment in mapped Desktop folder (persistent)
    Write-Host "Step 2: Creating virtual environment (if needed)..."
    $cachePath = "C:\Users\WDAGUtilityAccount\Desktop\wsb_cache"
    $venvPath = "C:\Users\WDAGUtilityAccount\Desktop\wsb_cache\venv"
    if (!(Test-Path $venvPath)) {
        Write-Host "Creating venv at: $venvPath"
        & $pythonExe -m venv $venvPath
    } else {
        Write-Host "Venv already exists at: $venvPath"
    }
    # Hide the folder to keep Desktop clean
    try {
        $item = Get-Item $cachePath -ErrorAction SilentlyContinue
        if ($item) {
            if (-not ($item.Attributes -band [IO.FileAttributes]::Hidden)) {
                $item.Attributes = $item.Attributes -bor [IO.FileAttributes]::Hidden
            }
        }
    } catch { }
    $venvPython = Join-Path $venvPath "Scripts\python.exe"
    if (!(Test-Path $venvPython)) {
        throw "Virtual environment Python not found at $venvPython"
    }
    Write-Host "Using venv Python: $venvPython"

    # Step 3: Install cua-computer-server into the venv
    Write-Host "Step 3: Installing cua-computer-server..."
    
    Write-Host "Upgrading pip..."
    & $venvPython -m pip install --upgrade pip --quiet
    
    Write-Host "Installing cua-computer-server..."
    & $venvPython -m pip install cua-computer-server
    
    Write-Host "cua-computer-server installation completed."

    # Step 4: Start computer server in background using the venv Python
    Write-Host "Step 4: Starting computer server in background..."
    Write-Host "Starting computer server with: $venvPython"
    
    # Start the computer server in the background
    $serverProcess = Start-Process -FilePath $venvPython -ArgumentList "-m", "computer_server.main" -WindowStyle Hidden -PassThru
    Write-Host "Computer server started in background with PID: $($serverProcess.Id)"
    
    # Give it a moment to start
    Start-Sleep -Seconds 3
    
    # Check if the process is still running
    if (Get-Process -Id $serverProcess.Id -ErrorAction SilentlyContinue) {
        Write-Host "Computer server is running successfully in background"
    } else {
        throw "Computer server failed to start or exited immediately"
    }

} catch {
    Write-Error "Setup failed: $_"
    Write-Host "Error details: $($_.Exception.Message)"
    Write-Host "Stack trace: $($_.ScriptStackTrace)"
    Write-Host ""
    Write-Host "Press any key to close this window..."
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    exit 1
}

Write-Host ""
Write-Host "Setup completed successfully!"
Write-Host "Press any key to close this window..."
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
