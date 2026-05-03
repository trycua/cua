#!/bin/bash
set -e

echo "Installing Git on Linux..."

# Update package manager with sudo
sudo apt-get update

# Install Git using apt package manager
sudo apt-get install -y git

# Verify installation
git --version

echo "Git installation completed successfully!"