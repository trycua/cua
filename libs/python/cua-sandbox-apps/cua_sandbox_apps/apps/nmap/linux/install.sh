#!/bin/bash
# Nmap installation script for Linux

set -e

echo "Installing Nmap..."

# Update package manager with sudo
sudo apt-get update -y || true

# Install Nmap from package manager with sudo
sudo apt-get install -y nmap

echo "Nmap installation complete!"

# Verify installation
which nmap
nmap --version