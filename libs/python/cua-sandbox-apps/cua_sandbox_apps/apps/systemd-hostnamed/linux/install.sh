#!/bin/bash
set -e

echo "Installing systemd-hostnamed..."

# Update package lists with sudo
sudo apt-get update

# Install systemd (includes systemd-hostnamed) with sudo
sudo apt-get install -y systemd

# Verify installation
if [ -f /lib/systemd/systemd-hostnamed ]; then
    echo "✓ systemd-hostnamed installed successfully"
else
    echo "✗ systemd-hostnamed binary not found"
    exit 1
fi

# Verify D-Bus service file exists
if [ -f /usr/share/dbus-1/system-services/org.freedesktop.hostname1.service ]; then
    echo "✓ D-Bus service file found"
else
    echo "✗ D-Bus service file not found"
    exit 1
fi

echo "Installation complete!"