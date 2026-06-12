#!/bin/bash
set -e

# DBeaver Installation Script for Linux
# This script installs DBeaver Community Edition by downloading and extracting the tarball

echo "Installing DBeaver Community Edition..."

# Install Java dependency
echo "Installing Java runtime..."
sudo apt-get update -qq
sudo apt-get install -y openjdk-11-jre-headless

# Create installation directory in home
INSTALL_DIR="$HOME/.local/opt/dbeaver"
mkdir -p "$INSTALL_DIR"

# Get the latest release URL from GitHub API
echo "Fetching latest DBeaver release..."
DOWNLOAD_URL=$(curl -s https://api.github.com/repos/dbeaver/dbeaver/releases/latest | grep -o 'https://github.com[^"]*x86_64.tar.gz' | head -1)

if [ -z "$DOWNLOAD_URL" ]; then
    echo "✗ Could not fetch DBeaver download URL"
    exit 1
fi

echo "Downloading DBeaver from: $DOWNLOAD_URL"
cd /tmp
curl -L -o dbeaver.tar.gz "$DOWNLOAD_URL"

# Extract to installation directory
echo "Extracting DBeaver..."
tar -xzf dbeaver.tar.gz -C "$INSTALL_DIR" --strip-components=1 || tar -xzf dbeaver.tar.gz -C "$INSTALL_DIR"

# Create symlink in ~/.local/bin for easy access
mkdir -p "$HOME/.local/bin"
ln -sf "$INSTALL_DIR/dbeaver" "$HOME/.local/bin/dbeaver"

# Make it executable
chmod +x "$INSTALL_DIR/dbeaver"

echo "DBeaver installation completed successfully!"

# Verify installation
if [ -f "$INSTALL_DIR/dbeaver" ]; then
    echo "✓ DBeaver binary found at $INSTALL_DIR/dbeaver"
    "$INSTALL_DIR/dbeaver" --version || true
else
    echo "✗ DBeaver binary not found"
    exit 1
fi