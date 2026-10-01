#!/bin/bash
set -e

echo "Launching Unity Hub..."

# Set required environment variables for Electron app
export DISPLAY=:0
export QT_QPA_PLATFORM=xcb

# Find the Unity Hub binary
UNITY_BIN="/usr/local/bin/unity-hub"

if [ ! -f "$UNITY_BIN" ] && [ ! -L "$UNITY_BIN" ]; then
    # Fallback to extracted version
    UNITY_BIN="$HOME/.local/bin/UnityHub/squashfs-root/unityhub"
fi

if [ ! -f "$UNITY_BIN" ]; then
    echo "Error: Unity Hub binary not found"
    exit 1
fi

# Launch Unity Hub
exec "$UNITY_BIN" "$@"
