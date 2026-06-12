#!/bin/bash
# Install script for GNU Emacs on Linux
set -e

echo "Installing GNU Emacs..."

# Update package manager cache
sudo apt-get update -qq

# Install Emacs (includes GTK support for GUI)
sudo apt-get install -y emacs

echo "✓ GNU Emacs installed successfully"