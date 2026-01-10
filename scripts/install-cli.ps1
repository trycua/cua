# Cua CLI Installation Script for Windows
$ErrorActionPreference = "Stop"

function Get-LatestCuaTag {
    $page = 1
    $perPage = 100
    $maxPages = 10

    while ($page -le $maxPages) {
        try {
            $response = Invoke-RestMethod -Uri "https://api.github.com/repos/trycua/cua/tags?per_page=$perPage&page=$page" -ErrorAction Stop

            if (-not $response -or $response.Count -eq 0) {
                return $null
            }

            $cuaTag = $response | Where-Object { $_.name -like "cua-*" } | Select-Object -First 1

            if ($cuaTag) {
                return $cuaTag.name
            }

            $page++
        } catch {
            return $null
        }
    }

    return $null
}

function Install-WithBun {
    Write-Host "Installing Cua CLI using Bun..." -ForegroundColor Yellow
    
    # Check if bun is already installed
    if (-not (Get-Command bun -ErrorAction SilentlyContinue)) {
        Write-Host "Installing Bun..." -ForegroundColor Yellow
        try {
            powershell -c "irm bun.sh/install.ps1|iex"
            
            # Refresh environment variables
            $env:Path = [System.Environment]::GetEnvironmentVariable("Path","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("Path","User")
            
            # Add bun to PATH for this session if not already there
            $bunPath = "$env:USERPROFILE\.bun\bin"
            if ($env:Path -notlike "*$bunPath*") {
                $env:Path = "$bunPath;$env:Path"
            }
        } catch {
            Write-Host "Error: Failed to install Bun. Please install manually from https://bun.sh" -ForegroundColor Red
            return $false
        }
    }

    # Verify bun installation
    if (-not (Get-Command bun -ErrorAction SilentlyContinue)) {
        Write-Host "Error: Bun installation failed. Please install manually from https://bun.sh" -ForegroundColor Red
        return $false
    }

    try {
        bun add -g @trycua/cli
        # Determine installed version from npm registry
        try {
            $bunVersion = (npm view @trycua/cli version) 2>$null
            if (-not $bunVersion) { $bunVersion = "unknown" }
        } catch { $bunVersion = "unknown" }
        # Ensure install dir and write version file
        $installDir = "$env:USERPROFILE\.cua\bin"
        if (-not (Test-Path $installDir)) { New-Item -ItemType Directory -Path $installDir -Force | Out-Null }
        Set-Content -Path (Join-Path $installDir ".version") -Value $bunVersion -NoNewline
        return $true
    } catch {
        Write-Host "Warning: Failed to install with Bun, trying npm..." -ForegroundColor Yellow
        try {
            npm install -g @trycua/cli
            # Determine installed version from npm registry
            try {
                $npmVersion = (npm view @trycua/cli version) 2>$null
                if (-not $npmVersion) { $npmVersion = "unknown" }
            } catch { $npmVersion = "unknown" }
            # Ensure install dir and write version file
            $installDir = "$env:USERPROFILE\.cua\bin"
            if (-not (Test-Path $installDir)) { New-Item -ItemType Directory -Path $installDir -Force | Out-Null }
            Set-Content -Path (Join-Path $installDir ".version") -Value $npmVersion -NoNewline
            return $true
        } catch {
            Write-Host "Error: Installation failed with npm as well." -ForegroundColor Red
            return $false
        }
    }
}

Write-Host "Installing Cua CLI..." -ForegroundColor Green

# Determine if this is a 64-bit system
$is64Bit = [Environment]::Is64BitOperatingSystem
if (-not $is64Bit) {
    Write-Host "Warning: 32-bit Windows is not supported. Falling back to Bun installation..." -ForegroundColor Yellow
    if (Install-WithBun) {
        exit 0
    } else {
        Write-Host "Error: Installation failed. Please try installing manually:" -ForegroundColor Red
        Write-Host "   irm https://cua.ai/install.ps1 | iex"
        exit 1
    }
}

# Get the latest cua release tag
$tagName = Get-LatestCuaTag
if (-not $tagName) {
    Write-Host "Warning: Could not find latest cua release, falling back to Bun installation" -ForegroundColor Yellow
    if (Install-WithBun) {
        exit 0
    } else {
        Write-Host "Error: Installation failed. Please try installing manually:" -ForegroundColor Red
        Write-Host "   irm https://cua.ai/install.ps1 | iex"
        exit 1
    }
}

# Extract version number (remove 'cua-v' prefix)
$version = $tagName -replace '^cua-v', ''

# Construct download URL using the specific cua release tag
$binaryUrl = "https://github.com/trycua/cua/releases/download/$tagName/cua-windows-x64.exe"

# Create installation directory
$installDir = "$env:USERPROFILE\.cua\bin"
if (-not (Test-Path $installDir)) {
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
}

$binaryPath = Join-Path $installDir "cua.exe"

# Download the binary
Write-Host "Downloading Cua CLI $version for Windows x64..." -ForegroundColor Cyan
try {
    Invoke-WebRequest -Uri $binaryUrl -OutFile $binaryPath -ErrorAction Stop
} catch {
    Write-Host "Warning: Failed to download pre-built binary, falling back to Bun installation" -ForegroundColor Yellow
    if (Install-WithBun) {
        exit 0
    } else {
        Write-Host "Error: Installation failed. Please try installing manually:" -ForegroundColor Red
        Write-Host "   irm https://cua.ai/install.ps1 | iex"
        exit 1
    }
}

# Write version file for binary install
try {
    Set-Content -Path (Join-Path $installDir ".version") -Value $version -NoNewline
} catch {
    # Non-fatal
}

# Add to PATH if not already there
$currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($currentPath -notlike "*$installDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$currentPath;$installDir", "User")
    $env:Path = "$env:Path;$installDir"
    Write-Host "Success: Added $installDir to your PATH" -ForegroundColor Green
}

# Verify installation
if (Test-Path $binaryPath) {
    Write-Host "Success: Cua CLI $version installed successfully to $binaryPath" -ForegroundColor Green
    Write-Host ""
    Write-Host "Get started with:" -ForegroundColor Cyan
    Write-Host "   cua login"
    Write-Host "   cua create --os linux --configuration small --region north-america"
    Write-Host ""
    Write-Host "For more help, visit: https://docs.cua.ai/libraries/cua-cli" -ForegroundColor Cyan
    
    # Offer to add to PATH if not already there
    if (-not ($env:Path -like "*$installDir*")) {
        Write-Host ""
        Write-Host "Note: Please restart your terminal or run the following command to use Cua CLI:" -ForegroundColor Yellow
        Write-Host "   `$env:Path += ';$installDir'"
    }
} else {
    Write-Host "Error: Installation failed. Please try installing manually:" -ForegroundColor Red
    Write-Host "   irm https://cua.ai/install.ps1 | iex"
    exit 1
}