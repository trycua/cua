#!/bin/bash
set -e

# Install Visual Studio Code on Linux (Ubuntu/Debian)
# This script is idempotent and handles existing installations

echo "Installing Visual Studio Code on Linux..."

# Update package lists
sudo apt-get update -y

# Install curl (needed for downloading the key) if not present
sudo apt-get install -y curl gpg

# Add Microsoft's GPG key (or update if it already exists)
curl https://packages.microsoft.com/keys/microsoft.asc | gpg --dearmor | sudo tee /usr/share/keyrings/microsoft.gpg > /dev/null

# Add or update the VS Code repository
echo 'deb [arch=amd64,arm64,armhf signed-by=/usr/share/keyrings/microsoft.gpg] https://packages.microsoft.com/repos/code stable main' | sudo tee /etc/apt/sources.list.d/vscode.list > /dev/null

# Update package lists again to include the repository
sudo apt-get update -y

# Install VS Code (or update if already installed)
sudo apt-get install -y code

echo "✅ Visual Studio Code installed successfully!"
echo "Binary path: $(which code)"