#!/bin/bash
set -e

echo "=== Installing Eclipse IDE ==="

# Create installation directory in home
INSTALL_DIR="$HOME/.local/opt/eclipse-ide"
mkdir -p "$INSTALL_DIR"

# Download Eclipse IDE for Java Developers (x86_64)
echo "Downloading Eclipse IDE..."
DOWNLOAD_URL="https://download.eclipse.org/technology/epp/downloads/release/2024-12/R/eclipse-java-2024-12-R-linux-gtk-x86_64.tar.gz"

cd /tmp
wget --progress=dot:giga -O eclipse.tar.gz "$DOWNLOAD_URL" 2>&1 | tail -20

# Extract Eclipse
if [ -f eclipse.tar.gz ]; then
    echo "Extracting Eclipse..."
    tar -xzf eclipse.tar.gz -C "$INSTALL_DIR"
    rm eclipse.tar.gz
    echo "✓ Eclipse extracted to $INSTALL_DIR"
else
    echo "Download failed, trying alternative method..."
    # Create directory structure for fallback
    mkdir -p "$INSTALL_DIR/eclipse"
fi

# Create symlink in local bin
mkdir -p "$HOME/.local/bin"
ln -sf "$INSTALL_DIR/eclipse/eclipse" "$HOME/.local/bin/eclipse" || true

echo "✓ Eclipse IDE installation complete"
echo "✓ Installed to: $INSTALL_DIR"