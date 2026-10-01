#!/bin/bash
set -e

echo "Installing Ansible..."

# Update package lists
sudo apt-get update -qq

# Try to install from apt repository first
if sudo apt-get install -y -qq ansible 2>/dev/null; then
    echo "✓ Ansible installed via apt"
    exit 0
fi

# Fallback: install via pip
echo "Installing via pip..."
sudo apt-get install -y -qq python3-pip
pip install -q ansible-core ansible

echo "✓ Ansible installed via pip"