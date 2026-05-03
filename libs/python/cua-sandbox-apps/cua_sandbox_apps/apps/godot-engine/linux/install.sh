#!/bin/bash
set -e

echo "Installing Godot Engine..."

# Install dependencies
sudo apt-get update -qq
sudo apt-get install -y wget > /dev/null 2>&1

# Create installation directory
INSTALL_DIR="/opt/godot"
sudo mkdir -p "$INSTALL_DIR"

# Get the latest Godot 4.x release download URL
# Using the latest stable release
GODOT_VERSION="4.3"
DOWNLOAD_URL="https://github.com/godotengine/godot/releases/download/${GODOT_VERSION}-stable/Godot_v${GODOT_VERSION}-stable_linux.x86_64.zip"

echo "Downloading Godot Engine v${GODOT_VERSION}..."
cd /tmp
wget -q "$DOWNLOAD_URL" -O godot.zip || {
    echo "Failed to download from version ${GODOT_VERSION}, trying latest..."
    DOWNLOAD_URL="https://github.com/godotengine/godot/releases/download/4.6-stable/Godot_v4.6-stable_linux.x86_64.zip"
    wget -q "$DOWNLOAD_URL" -O godot.zip
}

echo "Extracting Godot Engine..."
unzip -q godot.zip -d /tmp/godot_extract

# Find the executable
GODOT_BIN=$(find /tmp/godot_extract -name "Godot_v*" -o -name "godot*" | grep -E "Godot_v|^.*godot$" | head -1)

if [ -z "$GODOT_BIN" ]; then
    echo "Error: Could not find Godot executable"
    exit 1
fi

echo "Installing to $INSTALL_DIR..."
sudo cp "$GODOT_BIN" "$INSTALL_DIR/godot"
sudo chmod +x "$INSTALL_DIR/godot"

# Create symlink in /usr/local/bin for easy access
sudo ln -sf "$INSTALL_DIR/godot" /usr/local/bin/godot

# Clean up
rm -f /tmp/godot.zip
rm -rf /tmp/godot_extract

echo "Godot Engine installation completed successfully!"
echo "Run 'godot' or 'godot --help' to verify installation"