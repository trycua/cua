#!/bin/bash
set -e

echo "Installing Wireshark on Linux..."

# Update package index
sudo apt-get update

# Install wireshark package
# Use DEBIAN_FRONTEND=noninteractive to skip prompts about packet capture setup
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y wireshark

echo "✅ Wireshark installed successfully"