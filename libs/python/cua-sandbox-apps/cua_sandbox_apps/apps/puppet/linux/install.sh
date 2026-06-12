#!/bin/bash

# Puppet Installation Script for Linux
# This script installs Puppet Agent/Client from APT package repository

set -e

echo "========================================"
echo "Installing Puppet on Linux"
echo "========================================"

# Update package manager
echo "[1/4] Updating package manager..."
sudo apt-get update -y

# Install Puppet repository package
echo "[2/4] Adding Puppet repository..."
# The Puppet collection repository provides the latest stable releases
UBUNTU_CODENAME=$(lsb_release -cs)
PUPPET_RELEASE_PKG="puppet-release-${UBUNTU_CODENAME}.deb"

if ! apt-cache policy puppet &> /dev/null; then
    # Download and install the Puppet release package to add the repo
    cd /tmp
    wget -q https://apt.puppet.com/puppet-release-${UBUNTU_CODENAME}.deb -O puppet-release.deb || \
    wget -q https://apt.puppetlabs.com/puppet-release-${UBUNTU_CODENAME}.deb -O puppet-release.deb
    sudo dpkg -i puppet-release.deb || true
    sudo apt-get update -y
fi

# Install Puppet
echo "[3/4] Installing Puppet package..."
sudo apt-get install -y puppet

# Display installation information
echo "[4/4] Verifying installation..."
echo ""
echo "========================================"
echo "Installation Complete!"
echo "========================================"
echo ""

# Show installed version
if command -v puppet &> /dev/null; then
    echo "✓ Puppet installed successfully"
    puppet --version
else
    echo "✗ Puppet installation may have failed"
    exit 1
fi

echo ""
echo "Puppet binaries available:"
which puppet 2>/dev/null && echo "  - puppet" || true
which facter &>/dev/null && echo "  - facter" || true
which hiera &>/dev/null && echo "  - hiera" || true

echo ""
echo "To start using Puppet, try:"
echo "  puppet --help           # Show Puppet help"
echo "  puppet --version        # Show version"
echo "  facter                  # List system facts"
echo ""