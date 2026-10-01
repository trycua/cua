#!/bin/bash
set -e

echo "=== Installing GNU Awk (gawk) ==="

# Update package list
sudo apt-get update

# Install gawk
sudo apt-get install -y gawk

# Verify installation
echo "=== Verifying installation ==="
gawk --version

echo "=== GNU Awk installed successfully ==="