#!/bin/bash
set -e

echo "=== MySQL Server Install Script ==="
echo ""

# Update package lists
echo "Updating package lists..."
sudo apt-get update -qq

# Install MySQL Server (silent mode, accepting default configuration)
echo "Installing MySQL Server..."
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq mysql-server

# Verify installation
echo ""
echo "Verifying MySQL installation..."
mysql --version

echo ""
echo "=== MySQL installation complete ==="