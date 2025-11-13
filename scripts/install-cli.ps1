# CUA CLI Installation Script for Windows
$ErrorActionPreference = "Stop"

Write-Host "🚀 Installing CUA CLI..." -ForegroundColor Green

# Check if bun is already installed
try {
    $bunVersion = bun --version 2>$null
    Write-Host "✅ Bun is already installed (version $bunVersion)" -ForegroundColor Green
} catch {
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
        exit 1
    }
}

# Verify bun installation
try {
    $bunVersion = bun --version 2>$null
    Write-Host "✅ Bun verified (version $bunVersion)" -ForegroundColor Green
} catch {
    Write-Host "❌ Bun installation failed. Please install manually from https://bun.sh" -ForegroundColor Red
    exit 1
}

Write-Host "📦 Installing CUA CLI..." -ForegroundColor Yellow
try {
    bun add -g @trycua/cli
} catch {
    Write-Host "❌ Failed to install CUA CLI with Bun. Trying with npm..." -ForegroundColor Yellow
    try {
        npm install -g @trycua/cli
    } catch {
        Write-Host "❌ Installation failed. Please ensure you have Node.js or Bun installed." -ForegroundColor Red
        exit 1
    }
}

# Verify installation
try {
    $cuaVersion = cua --version 2>$null
    Write-Host "✅ CUA CLI installed successfully!" -ForegroundColor Green
    Write-Host ""
    Write-Host "🎉 Get started with:" -ForegroundColor Cyan
    Write-Host "   cua auth login" -ForegroundColor White
    Write-Host "   cua vm create --os linux --configuration small --region north-america" -ForegroundColor White
    Write-Host ""
    Write-Host "📚 For more help, visit: https://docs.cua.ai/libraries/cua-cli" -ForegroundColor Cyan
} catch {
    Write-Host "❌ Installation verification failed. Please try installing manually:" -ForegroundColor Red
    Write-Host "   npm install -g @trycua/cli" -ForegroundColor White
    exit 1
}
