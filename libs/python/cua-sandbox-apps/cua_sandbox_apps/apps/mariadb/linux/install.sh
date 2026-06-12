#!/bin/bash
set -e

echo "Installing MariaDB Server..."

# Update package lists
sudo apt-get update

# Install MariaDB Server (non-interactive)
export DEBIAN_FRONTEND=noninteractive
sudo apt-get install -y mariadb-server

# Verify installation
which mysqld
mysqld --version

echo "MariaDB installation completed successfully"