#!/bin/bash
set -e

# Create installation directory
sudo mkdir -p /opt/godot
cd /opt/godot

# Download latest Godot 4.x release
# Using the standard release URL pattern
GODOT_VERSION="4.3"
GODOT_URL="https://github.com/godotengine/godot/releases/download/${GODOT_VERSION}-stable/Godot_v${GODOT_VERSION}-stable_linux.x86_64.zip"

# Download the binary
sudo wget -O godot.zip "${GODOT_URL}" 2>&1 || {
  # Fallback: try without version patch
  GODOT_VERSION="4.2"
  GODOT_URL="https://github.com/godotengine/godot/releases/download/${GODOT_VERSION}-stable/Godot_v${GODOT_VERSION}-stable_linux.x86_64.zip"
  sudo wget -O godot.zip "${GODOT_URL}"
}

# Extract the binary
sudo unzip -o godot.zip

# Find the executable and make it executable
sudo find /opt/godot -name "Godot_v*" -type f -exec chmod +x {} \;

# Create a symlink in /usr/local/bin for easy access
GODOT_BIN=$(find /opt/godot -name "Godot_v*" -type f | head -1)
if [ -n "$GODOT_BIN" ]; then
  sudo ln -sf "$GODOT_BIN" /usr/local/bin/godot
fi

# Verify installation
/usr/local/bin/godot --version 2>/dev/null || echo "Godot installed at $GODOT_BIN"

echo "Godot Engine installed successfully"