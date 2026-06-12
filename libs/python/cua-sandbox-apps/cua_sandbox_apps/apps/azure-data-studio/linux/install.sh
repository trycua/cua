#!/bin/bash
# Install script for Azure Data Studio on Linux
# This script downloads and installs Azure Data Studio via the official .deb package

set -e

echo "Installing Azure Data Studio on Linux..."

# Create temporary directory for download
TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

# Change to temp directory
cd "$TEMP_DIR"

# Download the latest Azure Data Studio .deb package from Microsoft
echo "Downloading Azure Data Studio..."
wget -q "https://go.microsoft.com/fwlink/?linkid=2324720" -O azuredatastudio-linux.deb

# Update package lists
echo "Updating package lists..."
sudo -E apt-get update -qq

# Install any missing dependencies (optional but recommended)
echo "Installing dependencies..."
sudo -E apt-get install -y libxss1 libgconf-2-4 libunwind8 2>/dev/null || true

# Install the downloaded package
echo "Installing Azure Data Studio package..."
sudo -E dpkg -i azuredatastudio-linux.deb

# Fix any remaining dependency issues
sudo -E apt-get install -y -f 2>/dev/null || true

# Verify installation
if which azuredatastudio > /dev/null 2>&1; then
    echo "✓ Azure Data Studio installed successfully!"
    exit 0
else
    echo "✗ Installation verification failed"
    exit 1
fi