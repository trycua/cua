#!/bin/bash
set -e

# Apache NetBeans Installation Script for Linux
# This script installs Apache NetBeans 29 from official Apache distribution

echo "Installing Apache NetBeans..."

# Install dependencies (Java 21 and tools)
echo "Installing dependencies..."
sudo apt-get update
sudo apt-get install -y openjdk-21-jdk curl unzip

# Create installation directory
INSTALL_DIR="/opt/netbeans"
sudo mkdir -p "$INSTALL_DIR"

# Download NetBeans
echo "Downloading Apache NetBeans 29..."
cd /tmp
DOWNLOAD_URL="https://dlcdn.apache.org/netbeans/netbeans/29/netbeans-29-bin.zip"
curl -L -o netbeans-29-bin.zip "$DOWNLOAD_URL"

# Extract to installation directory
echo "Extracting NetBeans..."
sudo unzip -q netbeans-29-bin.zip -d "$INSTALL_DIR"

# Make the executable accessible via symlink
NETBEANS_BIN="$INSTALL_DIR/netbeans/bin/netbeans"
sudo ln -sf "$NETBEANS_BIN" /usr/local/bin/netbeans

# Clean up download
rm /tmp/netbeans-29-bin.zip

echo "✓ NetBeans installation complete!"
echo "✓ Launch with: netbeans"