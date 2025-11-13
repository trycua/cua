#!/bin/bash
set -e

# CUA CLI Installation Script for macOS/Linux
echo "🚀 Installing CUA CLI..."

# Check if bun is already installed
if command -v bun &> /dev/null; then
    echo "✅ Bun is already installed"
else
    echo "📦 Installing Bun..."
    curl -fsSL https://bun.sh/install | bash
    
    # Source the shell profile to make bun available
    if [ -f "$HOME/.bashrc" ]; then
        source "$HOME/.bashrc"
    elif [ -f "$HOME/.zshrc" ]; then
        source "$HOME/.zshrc"
    fi
    
    # Add bun to PATH for this session
    export PATH="$HOME/.bun/bin:$PATH"
fi

# Verify bun installation
if ! command -v bun &> /dev/null; then
    echo "❌ Failed to install Bun. Please install manually from https://bun.sh"
    exit 1
fi

echo "📦 Installing CUA CLI..."
bun add -g @trycua/cli

# Verify installation
if command -v cua &> /dev/null; then
    echo "✅ CUA CLI installed successfully!"
    echo ""
    echo "🎉 Get started with:"
    echo "   cua auth login"
    echo "   cua vm create --os linux --configuration small --region north-america"
    echo ""
    echo "📚 For more help, visit: https://docs.cua.ai/libraries/cua-cli"
else
    echo "❌ Installation failed. Please try installing manually:"
    echo "   npm install -g @trycua/cli"
    exit 1
fi
