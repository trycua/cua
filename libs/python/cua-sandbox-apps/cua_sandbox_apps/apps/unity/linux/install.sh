#!/bin/bash
set -e

echo "Installing Unity Hub on Linux..."

# Create installation directory
INSTALL_DIR="$HOME/.local/bin"
mkdir -p "$INSTALL_DIR"

# Download Unity Hub AppImage if not already present
APPIMAGE_URL="https://public-cdn.cloud.unity3d.com/hub/prod/UnityHub.AppImage"
APPIMAGE_PATH="$INSTALL_DIR/UnityHub.AppImage"

if [ ! -f "$APPIMAGE_PATH" ]; then
    echo "Downloading Unity Hub AppImage..."
    wget -q "$APPIMAGE_URL" -O "$APPIMAGE_PATH" || {
        echo "Failed to download Unity Hub AppImage"
        exit 1
    }
fi

# Make AppImage executable
chmod +x "$APPIMAGE_PATH"

# Extract AppImage to avoid FUSE requirement
EXTRACT_DIR="$INSTALL_DIR/UnityHub"
if [ ! -d "$EXTRACT_DIR" ]; then
    echo "Extracting Unity Hub AppImage..."
    mkdir -p "$EXTRACT_DIR"
    cd "$EXTRACT_DIR"
    "$APPIMAGE_PATH" --appimage-extract > /dev/null 2>&1 || true
    if [ ! -f "$EXTRACT_DIR/squashfs-root/unityhub" ]; then
        echo "Failed to extract AppImage"
        exit 1
    fi
fi

# Create symlink to the extracted binary
BINARY_PATH="$EXTRACT_DIR/squashfs-root/unityhub"
if [ ! -L "/usr/local/bin/unity-hub" ]; then
    sudo ln -sf "$BINARY_PATH" /usr/local/bin/unity-hub
fi

echo "Unity Hub installation completed successfully."
echo "To launch Unity Hub, run: unity-hub"
