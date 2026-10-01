#!/bin/bash
set -e

echo "=== PyCharm Installation Script ==="

# Install minimum required dependencies
echo "Installing minimal dependencies..."
sudo apt-get update -qq > /dev/null 2>&1
sudo apt-get install -y \
    libxi6 \
    libxrender1 \
    libxtst6 \
    libfontconfig1 \
    libgtk-3-0 \
    openjdk-11-jdk-headless \
    curl > /dev/null 2>&1

# Create installation directory
INSTALL_DIR="/opt/pycharm"
echo "Creating installation directory..."
sudo mkdir -p "$INSTALL_DIR"

# Download PyCharm Community Edition using curl with resume
echo "Downloading PyCharm Community Edition (this may take a few minutes)..."
DOWNLOAD_URL="https://download.jetbrains.com/python/pycharm-community-2024.1.tar.gz"
TEMP_FILE="/tmp/pycharm.tar.gz"

# Use curl for more reliable downloading
curl -L -C - "$DOWNLOAD_URL" -o "$TEMP_FILE" 2>&1 | grep -E "^  %|^Saved" || true

if [ ! -f "$TEMP_FILE" ] || [ ! -s "$TEMP_FILE" ]; then
    echo "ERROR: Failed to download PyCharm"
    exit 1
fi

echo "Downloaded $(du -h "$TEMP_FILE" | cut -f1)"

# Extract to installation directory
echo "Extracting PyCharm..."
sudo tar -xzf "$TEMP_FILE" -C "$INSTALL_DIR" --strip-components=1

# Clean up temp file
rm -f "$TEMP_FILE"

# Create symlink for easy access
echo "Creating symlink..."
sudo ln -sf "$INSTALL_DIR/bin/pycharm.sh" /usr/local/bin/pycharm 2>/dev/null || true

# Create .desktop entry for application menu
echo "Creating desktop entry..."
sudo mkdir -p /usr/share/applications
sudo tee /usr/share/applications/pycharm.desktop > /dev/null <<'EOF'
[Desktop Entry]
Type=Application
Name=PyCharm Community Edition
Comment=Professional IDE for Python
Exec=/opt/pycharm/bin/pycharm.sh %f
Icon=pycharm
Categories=Development;IDE;
Terminal=false
StartupNotify=true
EOF

echo "=== PyCharm installation complete ==="
echo "Binary location: /opt/pycharm/bin/pycharm.sh"
echo "Launch command: /opt/pycharm/bin/pycharm.sh"