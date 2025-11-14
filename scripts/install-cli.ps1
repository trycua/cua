# CUA CLI Installation Script for Windows
$ErrorActionPreference = "Stop"

function Install-WithBun {
    Write-Host "📦 Installing CUA CLI using Bun..." -ForegroundColor Yellow
    
    # Check if bun is already installed
    if (-not (Get-Command bun -ErrorAction SilentlyContinue)) {
        Write-Host "📦 Installing Bun..." -ForegroundColor Yellow
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
            Write-Host "❌ Failed to install Bun. Please install manually from https://bun.sh" -ForegroundColor Red
            return $false
        }
    }

    # Verify bun installation
    if (-not (Get-Command bun -ErrorAction SilentlyContinue)) {
        Write-Host "❌ Bun installation failed. Please install manually from https://bun.sh" -ForegroundColor Red
        return $false
    }

    try {
        bun add -g @trycua/cli
        return $true
    } catch {
        Write-Host "❌ Failed to install with Bun, trying npm..." -ForegroundColor Yellow
        try {
            npm install -g @trycua/cli
            return $true
        } catch {
            Write-Host "❌ Installation failed with npm as well." -ForegroundColor Red
            return $false
        }
    }
}

Write-Host "🚀 Installing CUA CLI..." -ForegroundColor Green

# Determine if this is a 64-bit system
$is64Bit = [Environment]::Is64BitOperatingSystem
if (-not $is64Bit) {
    Write-Host "⚠️  32-bit Windows is not supported. Falling back to Bun installation..." -ForegroundColor Yellow
    if (Install-WithBun) {
        exit 0
    } else {
        Write-Host "❌ Installation failed. Please try installing manually:" -ForegroundColor Red
        Write-Host "   irm https://cua.ai/install.ps1 | iex"
        exit 1
    }
}

# Get the latest release version
try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/trycua/cua/releases/latest" -ErrorAction Stop
    $version = $release.tag_name -replace '^cua-v', ''
    $binaryUrl = "https://github.com/trycua/cua/releases/download/$($release.tag_name)/cua-windows-x64.exe"
} catch {
    Write-Host "⚠️  Could not fetch latest release, falling back to Bun installation" -ForegroundColor Yellow
    if (Install-WithBun) {
        exit 0
    } else {
        Write-Host "❌ Installation failed. Please try installing manually:" -ForegroundColor Red
        Write-Host "   irm https://cua.ai/install.ps1 | iex"
        exit 1
    }
}

# Create installation directory
$installDir = "$env:USERPROFILE\.cua\bin"
if (-not (Test-Path $installDir)) {
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
}

$binaryPath = Join-Path $installDir "cua.exe"

# Download the binary
Write-Host "📥 Downloading CUA CLI $version for Windows x64..." -ForegroundColor Cyan
try {
    Invoke-WebRequest -Uri $binaryUrl -OutFile $binaryPath -ErrorAction Stop
} catch {
    Write-Host "⚠️  Failed to download pre-built binary, falling back to Bun installation" -ForegroundColor Yellow
    if (Install-WithBun) {
        exit 0
    } else {
        Write-Host "❌ Installation failed. Please try installing manually:" -ForegroundColor Red
        Write-Host "   irm https://cua.ai/install.ps1 | iex"
        exit 1
    }
}

# Add to PATH if not already there
$currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($currentPath -notlike "*$installDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$currentPath;$installDir", "User")
    $env:Path = "$env:Path;$installDir"
    Write-Host "✅ Added $installDir to your PATH" -ForegroundColor Green
}

# Verify installation
if (Test-Path $binaryPath) {
    Write-Host "✅ CUA CLI $version installed successfully to $binaryPath" -ForegroundColor Green
    Write-Host ""
    Write-Host "🎉 Get started with:" -ForegroundColor Cyan
    Write-Host "   cua auth login"
    Write-Host "   cua vm create --os linux --configuration small --region north-america"
    Write-Host ""
    Write-Host "📚 For more help, visit: https://docs.cua.ai/libraries/cua-cli" -ForegroundColor Cyan
    
    # Offer to add to PATH if not already there
    if (-not ($env:Path -like "*$installDir*")) {
        Write-Host ""
        Write-Host "⚠️  Please restart your terminal or run the following command to use CUA CLI:" -ForegroundColor Yellow
        Write-Host "   `$env:Path += ';$installDir'"
    }
} else {
    Write-Host "❌ Installation failed. Please try installing manually:" -ForegroundColor Red
    Write-Host "   irm https://cua.ai/install.ps1 | iex"
    exit 1
}
