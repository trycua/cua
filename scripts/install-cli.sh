#!/bin/bash
set -e

# CUA CLI Installation Script for macOS/Linux
echo "🚀 Installing CUA CLI..."

# Function to print success message
print_success() {
    local bin_path="$1"
    local version="$2"
    local config_file="$3"
    
    echo -e "\033[32m✅  CUA CLI $version was installed successfully to $bin_path\033[0m"
    echo -e "\033[90mAdded \"$bin_path\" to \$PATH in \"$config_file\"\033[0m"
    echo -e "\n\033[90mTo get started, run:\033[0m"
    echo -e "  source $config_file"
    echo -e "  cua --help\n"
    echo -e "\033[90m📚 For more help, visit: https://docs.cua.ai/libraries/cua-cli"
}

# Function to install with bun as fallback
install_with_bun() {
    echo "\033[90m📦 Installing CUA CLI using Bun...\033[0m"
    
    # Check if bun is already installed
    if ! command -v bun &> /dev/null; then
        echo "\033[90m📦 Installing Bun...\033[0m"
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
        echo "\033[90m❌ Failed to install Bun. Please install manually from https://bun.sh\033[0m"
        exit 1
    fi

    echo "\033[90m📦 Installing CUA CLI...\033[0m"
    if ! bun add -g @trycua/cli; then
        echo "\033[90m❌ Failed to install with Bun, trying npm...\033[0m"
        if ! npm install -g @trycua/cli; then
            echo "\033[90m❌ Installation failed. Please try installing manually:"
            echo "   npm install -g @trycua/cli"
            exit 1
        fi
    fi

    # Verify installation
    if command -v cua &> /dev/null; then
        # Determine which config file was updated
        local config_file="$HOME/.bashrc"
        if [ -f "$HOME/.zshrc" ]; then
            config_file="$HOME/.zshrc"
        elif [ -f "$HOME/.profile" ]; then
            config_file="$HOME/.profile"
        fi
        
        print_success "cua" "$VERSION" "$config_file"
        exit 0
    else
        echo "\033[90m❌ Installation failed. Please try installing manually:"
        echo "   npm install -g @trycua/cli"
        exit 1
    fi
}

# Determine OS and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

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
    echo "\033[90m⚠️  Pre-built binary not available for ${OS}-${ARCH}, falling back to Bun installation\033[0m"
    install_with_bun
    exit 0
fi

# Get the latest release version
LATEST_RELEASE=$(curl -s https://api.github.com/repos/trycua/cua/releases/latest)
if [ -z "$LATEST_RELEASE" ]; then
    echo "\033[90m⚠️  Could not fetch latest release, falling back to Bun installation\033[0m"
    install_with_bun
    exit 0
fi

# Extract version number (remove 'cua-v' prefix)
TAG_NAME=$(echo "$LATEST_RELEASE" | grep 'tag_name' | cut -d '"' -f 4)
VERSION=${TAG_NAME#cua-v}

# Find the binary URL in the release assets
BINARY_URL=$(echo "$LATEST_RELEASE" | grep -o 'https://.*/download/[^"]*/'${BINARY_NAME}'"' | head -1)

if [ -z "$BINARY_URL" ]; then
    echo "\033[90m⚠️  Could not find ${BINARY_NAME} in release assets, falling back to Bun installation\033[0m"
    install_with_bun
    exit 0
fi

# Create ~/.cua/bin directory if it doesn't exist
INSTALL_DIR="$HOME/.cua/bin"
mkdir -p "$INSTALL_DIR"

# Download the binary
echo "\033[90m📥 Downloading CUA CLI $VERSION for ${OS}-${ARCH}...\033[0m"
if ! curl -L "$BINARY_URL" -o "$INSTALL_DIR/cua" 2>/dev/null; then
    echo "\033[90m⚠️  Failed to download pre-built binary, falling back to Bun installation\033[0m"
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
        echo "\033[90mAdded $INSTALL_DIR to PATH in ~/.bashrc\033[0m"
    fi
    
    if [ -f "$HOME/.zshrc" ]; then
        echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$HOME/.zshrc"
        echo "\033[90mAdded $INSTALL_DIR to PATH in ~/.zshrc\033[0m"
    fi
    
    if [ -f "$HOME/.profile" ] && [ ! -f "$HOME/.bashrc" ] && [ ! -f "$HOME/.zshrc" ]; then
        echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$HOME/.profile"
        echo "\033[90mAdded $INSTALL_DIR to PATH in ~/.profile\033[0m"
    fi
    
    # Add to current session
    export PATH="$INSTALL_DIR:$PATH"
fi

# Verify installation
if command -v cua &> /dev/null; then
    # Determine which config file was updated
    config_file="$HOME/.bashrc"
    if [ -f "$HOME/.zshrc" ]; then
        config_file="$HOME/.zshrc"
    elif [ -f "$HOME/.profile" ]; then
        config_file="$HOME/.profile"
    fi
    
    print_success "$(which cua)" "$VERSION" "$config_file"
    exit 0
else
    echo "\033[90m❌ Installation failed. Please try installing manually:"
    echo "   curl -fsSL https://cua.ai/install.sh | sh"
    exit 1
fi
