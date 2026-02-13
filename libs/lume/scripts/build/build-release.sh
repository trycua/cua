#!/bin/sh

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Navigate to the lume root directory (two levels up from scripts/build/)
LUME_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$LUME_DIR"

swift build -c release --product lume
codesign --force --entitlement ./resources/lume.entitlements --sign - .build/release/lume

mkdir -p ./.release
cp -f .build/release/lume ./.release/lume

# Copy the resource bundle (contains unattended presets)
if [ -d ".build/release/lume_lume.bundle" ]; then
  cp -rf .build/release/lume_lume.bundle ./.release/
fi

# Install to user-local bin directory (standard location)
USER_BIN="$HOME/.local/bin"
mkdir -p "$USER_BIN"
cp -f ./.release/lume "$USER_BIN/lume"

# Install the resource bundle alongside the binary
if [ -d "./.release/lume_lume.bundle" ]; then
  cp -rf ./.release/lume_lume.bundle "$USER_BIN/"
fi

# Advise user to add to PATH if not present
if ! echo "$PATH" | grep -q "$USER_BIN"; then
  echo "[lume build] Note: $USER_BIN is not in your PATH. Add 'export PATH=\"$USER_BIN:\$PATH\"' to your shell profile."
fi
