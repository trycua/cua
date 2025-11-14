#!/bin/bash
set -e

# CUA CLI Installation Script for macOS/Linux
echo "🚀 Installing CUA CLI..."

# Determine OS and architecture
OS="$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH="$(uname -m)"

# Map architecture to the format used in release assets
case "$ARCH" in
    x86_64) ARCH="x64" ;;
    aarch64) ARCH="arm64" ;;
    arm64) ARCH="arm64" ;;
    *) ARCH="$ARCH" ;;
esac

# Determine the binary name
BINARY_NAME="cua-${OS}-${ARCH}"
if [ "$OS" = "darwin" ] && [ "$ARCH" = "arm64" ]; then
    BINARY_NAME="cua-darwin-arm64"
elif [ "$OS" = "darwin" ] && [ "$ARCH" = "x86_64" ]; then
    BINARY_NAME="cua-darwin-x64"
elif [ "$OS" = "linux" ] && [ "$ARCH" = "x86_64" ]; then
    BINARY_NAME="cua-linux-x64"
else
    echo "⚠️  Pre-built binary not available for ${OS}-${ARCH}, falling back to Bun installation"
    install_with_bun
    exit 0
fi

# Get the latest release version
LATEST_RELEASE=$(curl -s https://api.github.com/repos/trycua/cua/releases/latest)
if [ -z "$LATEST_RELEASE" ]; then
    echo "⚠️  Could not fetch latest release, falling back to Bun installation"
    install_with_bun
    exit 0
fi

# Extract version number (remove 'cua-v' prefix)
TAG_NAME=$(echo "$LATEST_RELEASE" | grep 'tag_name' | cut -d '"' -f 4)
VERSION=${TAG_NAME#cua-v}

# Find the binary URL in the release assets
BINARY_URL=$(echo "$LATEST_RELEASE" | grep -o "https://.*/download/[^"]*/${BINARY_NAME}" | head -1)

if [ -z "$BINARY_URL" ]; then
    echo "⚠️  Could not find ${BINARY_NAME} in release assets, falling back to Bun installation"
    install_with_bun
    exit 0
fi

# Create ~/.cua/bin directory if it doesn't exist
INSTALL_DIR="$HOME/.cua/bin"
mkdir -p "$INSTALL_DIR"

# Download the binary
echo "📥 Downloading CUA CLI $VERSION for ${OS}-${ARCH}..."
if ! curl -L "$BINARY_URL" -o "$INSTALL_DIR/cua" 2>/dev/null; then
    echo "⚠️  Failed to download pre-built binary, falling back to Bun installation"
    install_with_bun
    exit 0
fi

# Make the binary executable
chmod +x "$INSTALL_DIR/cua"

# Add ~/.cua/bin to PATH if not already in PATH
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    # Add to .bashrc, .zshrc, or .profile
    if [ -f "$HOME/.bashrc" ]; then
        echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$HOME/.bashrc"
        echo "Added $INSTALL_DIR to PATH in ~/.bashrc"
    fi
    
    if [ -f "$HOME/.zshrc" ]; then
        echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$HOME/.zshrc"
        echo "Added $INSTALL_DIR to PATH in ~/.zshrc"
    fi
    
    if [ -f "$HOME/.profile" ] && [ ! -f "$HOME/.bashrc" ] && [ ! -f "$HOME/.zshrc" ]; then
        echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$HOME/.profile"
        echo "Added $INSTALL_DIR to PATH in ~/.profile"
    fi
    
    # Add to current session
    export PATH="$INSTALL_DIR:$PATH"
fi

# Verify installation
if command -v cua &> /dev/null; then
    echo "✅ CUA CLI $VERSION installed successfully to $INSTALL_DIR/cua"
    echo ""
    echo "🎉 Get started with:"
    echo "   cua auth login"
    echo "   cua vm create --os linux --configuration small --region north-america"
    echo ""
    echo "📚 For more help, visit: https://docs.cua.ai/libraries/cua-cli"
else
    echo "❌ Installation failed. Please try installing manually:"
    echo "   curl -fsSL https://cua.ai/install.sh | sh"
    exit 1
fi

# Function to install with bun as fallback
install_with_bun() {
    echo "📦 Installing CUA CLI using Bun..."
    
    # Check if bun is already installed
    if ! command -v bun &> /dev/null; then
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
    if ! bun add -g @trycua/cli; then
        echo "❌ Failed to install with Bun, trying npm..."
        if ! npm install -g @trycua/cli; then
            echo "❌ Installation failed. Please try installing manually:"
            echo "   npm install -g @trycua/cli"
            exit 1
        fi
    fi

    # Verify installation
    if command -v cua &> /dev/null; then
        echo "✅ CUA CLI installed successfully using Bun!"
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
}
