#!/bin/bash
set -e

echo "Updating package manager..."
apt-get update -qq

echo "Installing Neovim..."
apt-get install -y neovim

echo "Installation complete!"
which nvim || echo "Warning: nvim not found in PATH"
nvim --version | head -1
