#!/bin/bash
set -e

# Install Chef CLI via Ruby gems
# Chef is a configuration management and infrastructure automation platform

echo "Installing Chef CLI..."

# Ensure Ruby and build tools are installed
sudo apt-get update
sudo apt-get install -y ruby ruby-dev build-essential git

# Install chef-cli gem which provides the chef-cli executable
# Using --no-document to skip documentation installation for faster installation
sudo gem install chef-cli --no-document

# Ensure /usr/local/bin is in PATH
export PATH="/usr/local/bin:$PATH"

echo "Chef CLI installation complete!"
/usr/local/bin/chef-cli --version