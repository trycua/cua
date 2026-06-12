#!/bin/bash
set -e

echo "Installing IntelliJ IDEA Community Edition..."

# Install dependencies
echo "Installing dependencies..."
sudo apt-get update -qq
sudo apt-get install -y -qq wget curl default-jre

# Create installation directory
INSTALL_DIR="/opt/intellij-idea"
echo "Creating installation directory: $INSTALL_DIR"
sudo mkdir -p "$INSTALL_DIR"

# Download IntelliJ IDEA
echo "Downloading IntelliJ IDEA Community Edition (1.1GB - this may take several minutes)..."
TEMP_DIR=$(mktemp -d)
cd "$TEMP_DIR"

# Download using wget (reliable for large files)
wget "https://data.services.jetbrains.com/products/download?code=IIC&platform=linux" -O idea.tar.gz

# Verify download
if [ ! -f idea.tar.gz ] || [ ! -s idea.tar.gz ]; then
    echo "Error: Download failed"
    exit 1
fi

echo "Download complete. Extracting..."

# Extract
tar -xzf idea.tar.gz

# Find extracted directory
EXTRACTED_DIR=$(ls -d idea-* 2>/dev/null | head -1)
if [ -z "$EXTRACTED_DIR" ]; then
    echo "Error: Could not find extracted directory"
    exit 1
fi

echo "Installing to $INSTALL_DIR..."

# Clean and copy
sudo rm -rf "${INSTALL_DIR:?}"/*
sudo cp -r "$EXTRACTED_DIR"/* "$INSTALL_DIR/"

echo "Setting up symlinks and permissions..."

# Create symlink
sudo ln -sf "$INSTALL_DIR/bin/idea.sh" /usr/local/bin/idea

# Make executables
sudo chmod +x "$INSTALL_DIR/bin/idea.sh"

echo "Creating desktop entry..."

# Desktop entry
sudo mkdir -p /usr/share/applications
sudo sh -c 'cat > /usr/share/applications/jetbrains-idea.desktop' << 'EOF'
[Desktop Entry]
Type=Application
Name=IntelliJ IDEA Community Edition
Exec=/opt/intellij-idea/bin/idea.sh %f
Icon=/opt/intellij-idea/bin/idea.png
Categories=Development;IDE;Java;
Terminal=false
StartupNotify=true
Comment=A professional IDE for Java and Kotlin development
EOF

sudo chmod 644 /usr/share/applications/jetbrains-idea.desktop

# Cleanup
cd /
rm -rf "$TEMP_DIR"

echo "✅ Installation complete!"
echo "   Installed to: $INSTALL_DIR"
echo "   Launch with: idea"