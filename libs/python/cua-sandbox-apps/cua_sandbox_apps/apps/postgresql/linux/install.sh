#!/bin/bash
set -e

echo "Installing PostgreSQL..."

# Update package list
sudo apt-get update

# Install PostgreSQL (latest version from distribution repos)
# This will install postgresql and postgresql-contrib
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y postgresql postgresql-contrib

echo "PostgreSQL installed successfully!"

# Verify installation
psql --version

echo "Installation complete!"