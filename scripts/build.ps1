# PowerShell Build Script for CUA
# Exit on error
$ErrorActionPreference = "Stop"

# Colors for output
$RED = "Red"
$GREEN = "Green"
$BLUE = "Blue"

# Function to print step information
function Print-Step {
    param([string]$Message)
    Write-Host "==> $Message" -ForegroundColor $BLUE
}

# Function to print success message
function Print-Success {
    param([string]$Message)
    Write-Host "==> Success: $Message" -ForegroundColor $GREEN
}

# Function to print error message
function Print-Error {
    param([string]$Message)
    Write-Host "==> Error: $Message" -ForegroundColor $RED
}

# Get the script's directory and project root
$SCRIPT_DIR = Split-Path -Parent $MyInvocation.MyCommand.Path
$PROJECT_ROOT = Split-Path -Parent $SCRIPT_DIR

# Change to project root
Set-Location $PROJECT_ROOT

# Load environment variables from .env.local
if (Test-Path ".env.local") {
    Print-Step "Loading environment variables from .env.local..."
    Get-Content ".env.local" | ForEach-Object {
        if ($_ -match "^([^#][^=]*?)=(.*)$") {
            [Environment]::SetEnvironmentVariable($matches[1], $matches[2], "Process")
        }
    }
    Print-Success "Environment variables loaded"
} else {
    Print-Error ".env.local file not found"
    exit 1
}

# Check if conda is available
try {
    conda --version | Out-Null
    Print-Success "Conda is available"
} catch {
    Print-Error "Conda is not available. Please install Anaconda or Miniconda first."
    exit 1
}

# Create or update conda environment
Print-Step "Creating/updating conda environment 'cua' with Python 3.12..."
try {
    # Check if environment exists
    $envExists = conda env list | Select-String "^cua\s"
    if ($envExists) {
        Print-Step "Environment 'cua' already exists. Updating..."
        conda env update -n cua -f environment.yml --prune
    } else {
        Print-Step "Creating new environment 'cua'..."
        conda create -n cua python=3.12 -y
    }
    Print-Success "Conda environment 'cua' ready"
} catch {
    Print-Error "Failed to create/update conda environment"
    exit 1
}

# Activate conda environment
Print-Step "Activating conda environment 'cua'..."
try {
    conda activate cua
    Print-Success "Environment activated"
} catch {
    Print-Error "Failed to activate conda environment 'cua'"
    Print-Step "Please run: conda activate cua"
    Print-Step "Then re-run this script"
    exit 1
}

# Clean up existing environments and cache
Print-Step "Cleaning up existing environments..."
Get-ChildItem -Path . -Recurse -Directory -Name "__pycache__" | ForEach-Object { Remove-Item -Path $_ -Recurse -Force }
Get-ChildItem -Path . -Recurse -Directory -Name ".pytest_cache" | ForEach-Object { Remove-Item -Path $_ -Recurse -Force }
Get-ChildItem -Path . -Recurse -Directory -Name "dist" | ForEach-Object { Remove-Item -Path $_ -Recurse -Force }
Get-ChildItem -Path . -Recurse -Directory -Name "*.egg-info" | ForEach-Object { Remove-Item -Path $_ -Recurse -Force }

# Function to install a package and its dependencies
function Install-Package {
    param(
        [string]$PackageDir,
        [string]$PackageName,
        [string]$Extras = ""
    )
    
    Print-Step "Installing $PackageName..."
    Set-Location $PackageDir
    
    if (Test-Path "pyproject.toml") {
        if ($Extras) {
            pip install -e ".[$Extras]"
        } else {
            pip install -e .
        }
    } else {
        Print-Error "No pyproject.toml found in $PackageDir"
        Set-Location $PROJECT_ROOT
        return $false
    }
    
    Set-Location $PROJECT_ROOT
    return $true
}

# Install packages in order of dependency
Print-Step "Installing packages in development mode..."

# Install core first (base package with telemetry support)
if (-not (Install-Package "libs/python/core" "core")) { exit 1 }

# Install computer with all its dependencies and extras
if (-not (Install-Package "libs/python/computer" "computer" "all")) { exit 1 }

# Install omniparser
if (-not (Install-Package "libs/python/som" "som")) { exit 1 }

# Install agent with all its dependencies and extras
if (-not (Install-Package "libs/python/agent" "agent" "all")) { exit 1 }

# Install computer-server
if (-not (Install-Package "libs/python/computer-server" "computer-server")) { exit 1 }

# Install mcp-server
if (-not (Install-Package "libs/python/mcp-server" "mcp-server")) { exit 1 }

# Install development tools from root project
Print-Step "Installing development dependencies..."
pip install -e ".[dev,test,docs]"

# Create a .env file for VS Code to use the virtual environment
Print-Step "Creating .env file for VS Code..."
$pythonPath = "$PROJECT_ROOT/libs/python/core;$PROJECT_ROOT/libs/python/computer;$PROJECT_ROOT/libs/python/agent;$PROJECT_ROOT/libs/python/som;$PROJECT_ROOT/libs/python/computer-server;$PROJECT_ROOT/libs/python/mcp-server"
"PYTHONPATH=$pythonPath" | Out-File -FilePath ".env" -Encoding UTF8

Print-Success "All packages installed successfully!"
Print-Step "Your conda environment 'cua' is ready. To activate it:"
Write-Host "  conda activate cua" -ForegroundColor Yellow
