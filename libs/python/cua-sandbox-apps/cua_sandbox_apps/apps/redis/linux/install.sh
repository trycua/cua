#!/bin/bash
set -e

echo "Installing Redis..."

# Update package manager
sudo apt-get update

# Install Redis from apt repository
sudo apt-get install -y redis-server

# Enable redis service to start on boot
sudo systemctl enable redis-server || true

# Verify installation
redis-server --version

echo "Redis installation completed successfully!"