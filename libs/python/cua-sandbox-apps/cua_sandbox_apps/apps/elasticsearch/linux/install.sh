#!/bin/bash
set -e

echo "====== Installing Elasticsearch on Ubuntu/Debian ======"

# Update package list
echo "Updating package lists..."
apt-get update -qq

# Install prerequisites
echo "Installing prerequisites..."
apt-get install -y -qq apt-transport-https curl gpg wget

# Import the Elasticsearch PGP signing key (non-interactive)
echo "Importing Elasticsearch PGP key..."
curl -s https://artifacts.elastic.co/GPG-KEY-elasticsearch | gpg --batch --yes --dearmor -o /usr/share/keyrings/elasticsearch-keyring.gpg 2>/dev/null || true

# Configure the APT repository
echo "Configuring APT repository..."
echo "deb [signed-by=/usr/share/keyrings/elasticsearch-keyring.gpg] https://artifacts.elastic.co/packages/9.x/apt stable main" | tee /etc/apt/sources.list.d/elastic-9.x.list > /dev/null

# Update package lists again to include Elasticsearch repo
echo "Updating package lists with Elasticsearch repo..."
apt-get update -qq

# Install Elasticsearch
echo "Installing Elasticsearch..."
apt-get install -y -qq elasticsearch

# Set up systemd service
echo "Setting up systemd service..."
systemctl daemon-reload
systemctl enable elasticsearch.service

echo "====== Elasticsearch installation complete ======"
