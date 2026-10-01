#!/bin/bash
set -e

echo "=== Raneto Installation Script ==="

# Check if Node.js 20+ is already installed
NODE_VERSION=$(node -v 2>/dev/null | sed 's/v//' | cut -d. -f1)
if [ "$NODE_VERSION" -lt 18 ]; then
  echo "Upgrading Node.js..."
  curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -
  sudo apt-get remove -y nodejs libnode-dev 2>/dev/null || true
  sudo apt-get autoremove -y 2>/dev/null || true
  sudo apt-get install -y nodejs
fi

echo "Node.js version: $(node --version)"
echo "npm version: $(npm --version)"

# Use home directory for installation
INSTALL_DIR="$HOME/raneto"
echo "Installing Raneto to: $INSTALL_DIR"

# Clean and create directory
rm -rf "$INSTALL_DIR"
mkdir -p "$INSTALL_DIR"
cd "$INSTALL_DIR"

# Clone repository
echo "Cloning Raneto..."
git clone --depth 1 https://github.com/ryanlelek/Raneto.git .

# Install dependencies quickly
echo "Installing dependencies..."
npm install --production --no-save

# Create sample content
mkdir -p pages
echo "# Welcome to Raneto" > pages/index.md
echo "" >> pages/index.md
echo "This is a markdown-based knowledge base." >> pages/index.md

echo "=== Installation complete! ==="
echo "Installation directory: $INSTALL_DIR"
echo "To start: cd $INSTALL_DIR && npm start"