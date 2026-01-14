#!/bin/bash
set -e

# Cua CLI Installation Script for macOS/Linux
echo "🚀 Installing Cua CLI..."

# Function to print success message
print_success() {
    local bin_path="$1"
    local version="$2"
    local config_file="$3"
    
    printf "\033[32m✅  Cua CLI %s was installed successfully to %s\033[0m\n" "$version" "$bin_path"
    printf "\033[90mAdded \"%s\" to \$PATH in \"%s\"\033[0m\n" "$bin_path" "$config_file"
    printf "\n\033[90mTo get started, run:\033[0m\n"
    printf "  source %s\n" "$config_file"
    printf "  cua --help\n"
    printf "\033[90m📚 For more help, visit: https://docs.cua.ai/libraries/cua-cli\033[0m\n"
}

# Helper function to add line to profile if not present (idempotent)
add_to_profile_if_missing() {
    local file="$1"
    local line="$2"
    local description="$3"

    if [ -f "$file" ]; then
        if ! grep -qF "$line" "$file" 2>/dev/null; then
            echo "$line" >> "$file"
            echo "$description"
        fi
    fi
}

# Install shell completions (idempotent)
install_completions_for_path() {
    local cua_bin="$1"
    local COMPLETIONS_DIR="$HOME/.cua/completions"
    mkdir -p "$COMPLETIONS_DIR"

    # Generate completions script
    local COMPLETION_SCRIPT="$COMPLETIONS_DIR/cua.bash"
    "$cua_bin" completion > "$COMPLETION_SCRIPT" 2>/dev/null || true

    if [ -s "$COMPLETION_SCRIPT" ]; then
        local SOURCE_LINE="[ -f \"$COMPLETION_SCRIPT\" ] && source \"$COMPLETION_SCRIPT\""

        add_to_profile_if_missing "$HOME/.bashrc" "$SOURCE_LINE" "Added shell completions to ~/.bashrc"

        # For zsh, also add to .zshrc
        if [ -f "$HOME/.zshrc" ]; then
            add_to_profile_if_missing "$HOME/.zshrc" "$SOURCE_LINE" "Added shell completions to ~/.zshrc"
        fi
    fi
}

# Function to install with bun as fallback
install_with_bun() {
    echo "📦 Installing Cua CLI using Bun..."

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

    echo "📦 Installing Cua CLI..."
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
        # Determine which config file was updated
        local config_file="$HOME/.bashrc"
        if [ -f "$HOME/.zshrc" ]; then
            config_file="$HOME/.zshrc"
        elif [ -f "$HOME/.profile" ]; then
            config_file="$HOME/.profile"
        fi
        # Determine installed version via npm registry (fallback to unknown)
        local VERSION_BUN
        VERSION_BUN=$(npm view @trycua/cli version 2>/dev/null || echo "unknown")
        # Write version file to ~/.cua/bin/.version
        local INSTALL_DIR="$HOME/.cua/bin"
        mkdir -p "$INSTALL_DIR"
        echo "$VERSION_BUN" > "$INSTALL_DIR/.version"
        # Install completions
        install_completions_for_path "$(command -v cua)"
        # Print success and exit
        print_success "$(command -v cua)" "$VERSION_BUN" "$config_file"
        exit 0
    else
        echo "❌ Installation failed. Please try installing manually:"
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

# Function to get the latest cua release tag
get_latest_cua_tag() {
    local page=1
    local per_page=100
    local max_pages=10
    local CUA_TAG=""

    while [ $page -le $max_pages ]; do
        local response=$(curl -s "https://api.github.com/repos/trycua/cua/tags?per_page=$per_page&page=$page")

        if [ -z "$response" ] || [ "$(echo "$response" | grep -c '"name":')" -eq 0 ]; then
            return 1
        fi

        CUA_TAG=$(echo "$response" | grep '"name": "cua-' | head -n 1 | cut -d '"' -f 4)

        if [ -n "$CUA_TAG" ]; then
            echo "$CUA_TAG"
            return 0
        fi

        page=$((page + 1))
    done

    return 1
}

# Determine the binary name
BINARY_NAME="cua-${OS}-${ARCH}"
if [ "$OS" = "darwin" ] && [ "$ARCH" = "arm64" ]; then
    BINARY_NAME="cua-darwin-arm64"
elif [ "$OS" = "darwin" ] && [ "$ARCH" = "x64" ]; then
    BINARY_NAME="cua-darwin-x64"
elif [ "$OS" = "linux" ] && [ "$ARCH" = "x64" ]; then
    BINARY_NAME="cua-linux-x64"
else
    echo "⚠️  Pre-built binary not available for ${OS}-${ARCH}, falling back to Bun installation"
    install_with_bun
    exit 0
fi

# Get the latest cua release tag
TAG_NAME=$(get_latest_cua_tag)
if [ -z "$TAG_NAME" ]; then
    echo "⚠️  Could not find latest cua release, falling back to Bun installation"
    install_with_bun
    exit 0
fi

# Extract version number (remove 'cua-v' prefix)
VERSION=${TAG_NAME#cua-v}

# Construct download URL using the specific cua release tag
BINARY_URL="https://github.com/trycua/cua/releases/download/${TAG_NAME}/${BINARY_NAME}"
printf "\033[90mBINARY_URL: %s\033[0m\n" "$BINARY_URL"

# Create ~/.cua/bin directory if it doesn't exist
INSTALL_DIR="$HOME/.cua/bin"
mkdir -p "$INSTALL_DIR"

# Download the binary
echo "📥 Downloading Cua CLI $VERSION for ${OS}-${ARCH}..."
echo "📍 Downloading from: $BINARY_URL"

# Download with progress bar and proper error handling
if ! curl -L --progress-bar --fail "$BINARY_URL" -o "$INSTALL_DIR/cua"; then
    echo "❌ Failed to download pre-built binary from $BINARY_URL"
    echo "⚠️  Falling back to Bun installation"
    install_with_bun
    exit 0
fi

# Verify the downloaded file exists and has content
if [ ! -f "$INSTALL_DIR/cua" ] || [ ! -s "$INSTALL_DIR/cua" ]; then
    echo "❌ Downloaded file is missing or empty"
    echo "⚠️  Falling back to Bun installation"
    rm -f "$INSTALL_DIR/cua"
    install_with_bun
    exit 0
fi

# Check if the downloaded file looks like a binary (not HTML error page)
if file "$INSTALL_DIR/cua" | grep -q "HTML\|text"; then
    echo "❌ Downloaded file appears to be corrupted (HTML/text instead of binary)"
    echo "⚠️  Falling back to Bun installation"
    rm -f "$INSTALL_DIR/cua"
    install_with_bun
    exit 0
fi

# Make the binary executable
chmod +x "$INSTALL_DIR/cua"

# Write version file
echo "$VERSION" > "$INSTALL_DIR/.version"

# Add ~/.cua/bin to PATH if not already in PATH (idempotent)
PATH_EXPORT="export PATH=\"$INSTALL_DIR:\$PATH\""

if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    add_to_profile_if_missing "$HOME/.bashrc" "$PATH_EXPORT" "Added $INSTALL_DIR to PATH in ~/.bashrc"
    add_to_profile_if_missing "$HOME/.zshrc" "$PATH_EXPORT" "Added $INSTALL_DIR to PATH in ~/.zshrc"

    if [ ! -f "$HOME/.bashrc" ] && [ ! -f "$HOME/.zshrc" ]; then
        add_to_profile_if_missing "$HOME/.profile" "$PATH_EXPORT" "Added $INSTALL_DIR to PATH in ~/.profile"
    fi

    # Add to current session
    export PATH="$INSTALL_DIR:$PATH"
fi

# Install shell completions
install_completions_for_path "$INSTALL_DIR/cua"

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
    echo "❌ Installation failed. Please try installing manually:"
    echo "   curl -fsSL https://cua.ai/install.sh | sh"
    exit 1
fi
