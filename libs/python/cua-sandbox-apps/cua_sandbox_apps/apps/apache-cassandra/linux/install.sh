#!/bin/bash
set -e

# Apache Cassandra Installation Script for Linux
# This script installs Cassandra from binary tarball

echo "=== Apache Cassandra Installation ==="

# Update system packages (use sudo if needed)
sudo apt-get update -qq
sudo apt-get install -y -qq openjdk-11-jdk-headless curl wget

# Create cassandra user and group if they don't exist
if ! id -u cassandra > /dev/null 2>&1; then
    sudo useradd -m -d /var/lib/cassandra cassandra || true
fi

# Define installation paths
CASSANDRA_HOME="/opt/cassandra"
CASSANDRA_USER="cassandra"
DOWNLOAD_URL="https://www.apache.org/dyn/closer.lua/cassandra/5.0.7/apache-cassandra-5.0.7-bin.tar.gz"
MIRROR_URL="https://archive.apache.org/dist/cassandra/5.0.7/apache-cassandra-5.0.7-bin.tar.gz"

# Create installation directory
sudo mkdir -p "$CASSANDRA_HOME"

# Download and extract Cassandra
echo "Downloading Apache Cassandra 5.0.7..."
if command -v wget &> /dev/null; then
    wget -q "$MIRROR_URL" -O /tmp/cassandra.tar.gz || wget -q "$DOWNLOAD_URL" -O /tmp/cassandra.tar.gz
elif command -v curl &> /dev/null; then
    curl -s "$MIRROR_URL" -o /tmp/cassandra.tar.gz || curl -s "$DOWNLOAD_URL" -o /tmp/cassandra.tar.gz
else
    echo "Error: wget or curl not found"
    exit 1
fi

echo "Extracting Cassandra..."
sudo tar -xzf /tmp/cassandra.tar.gz -C "$CASSANDRA_HOME" --strip-components=1

# Create necessary directories
sudo mkdir -p /var/lib/cassandra
sudo mkdir -p /var/log/cassandra
sudo mkdir -p /etc/cassandra

# Copy default configuration
sudo cp "$CASSANDRA_HOME/conf/cassandra.yaml" /etc/cassandra/cassandra.yaml || true
sudo cp "$CASSANDRA_HOME/conf/cassandra-env.sh" /etc/cassandra/cassandra-env.sh || true

# Fix permissions
sudo chown -R "$CASSANDRA_USER:$CASSANDRA_USER" /var/lib/cassandra
sudo chown -R "$CASSANDRA_USER:$CASSANDRA_USER" /var/log/cassandra
sudo chown -R "$CASSANDRA_USER:$CASSANDRA_USER" /etc/cassandra
sudo chown -R "$CASSANDRA_USER:$CASSANDRA_USER" "$CASSANDRA_HOME"

# Create symbolic link in standard location
sudo ln -sf "$CASSANDRA_HOME/bin/cassandra" /usr/local/bin/cassandra || true

# Create startup configuration directory
sudo mkdir -p /etc/cassandra/conf.d

# Cleanup
rm -f /tmp/cassandra.tar.gz

echo "=== Installation Complete ==="
echo "Cassandra installed to: $CASSANDRA_HOME"
echo "Configuration: /etc/cassandra/"
echo "Data directory: /var/lib/cassandra"
echo "Log directory: /var/log/cassandra"