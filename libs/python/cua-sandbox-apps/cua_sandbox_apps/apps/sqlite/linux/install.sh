#!/bin/bash
set -e

# Update package lists
sudo apt-get update

# Install SQLite3 command-line tool and development libraries
sudo apt-get install -y sqlite3

# Verify installation
which sqlite3
sqlite3 --version

echo "SQLite installation complete!"