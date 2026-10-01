#!/bin/bash
set -e

# Install Vim on Linux
# This script installs both vim (CLI) and gvim (GUI) variants

echo "Updating package lists..."
sudo apt-get update

echo "Installing Vim and gvim..."
sudo apt-get install -y vim vim-gtk3

echo "Verifying installation..."
vim --version | head -1

echo "Installation complete!"