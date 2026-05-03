#!/bin/bash
set -e

echo "================================"
echo "Wazuh Agent Installation"
echo "================================"

# Update system packages
echo "[1/4] Updating system packages..."
sudo apt-get update -qq 2>&1 | tail -3
sudo apt-get install -y -qq curl wget gnupg2 ca-certificates apt-transport-https lsb-release 2>&1 | tail -3

# Add Wazuh repository
echo "[2/4] Adding Wazuh repository..."
curl -s https://packages.wazuh.com/key/GPG-KEY-WAZUH | sudo gpg --no-default-keyring --keyring gnupg-ring:/usr/share/keyrings/wazuh.gpg --import 2>&1 | tail -2
sudo chmod 644 /usr/share/keyrings/wazuh.gpg
echo "deb [signed-by=/usr/share/keyrings/wazuh.gpg] https://packages.wazuh.com/4.x/apt/ stable main" | sudo tee -a /etc/apt/sources.list.d/wazuh.list > /dev/null

# Update package list
echo "[3/4] Updating package cache..."
sudo apt-get update -qq 2>&1 | tail -3

# Install Wazuh Agent with a dummy manager address (for demo purposes)
echo "[4/4] Installing Wazuh Agent..."
sudo WAZUH_MANAGER="127.0.0.1" WAZUH_AGENT_NAME="demo-agent" WAZUH_AGENT_GROUP="default" apt-get install -y -qq wazuh-agent 2>&1 | tail -10

echo "================================"
echo "Wazuh Agent installation complete!"
echo "================================"
echo ""
echo "Wazuh Agent installed successfully"
echo "Location: /var/ossec/"
echo "Configuration: /var/ossec/etc/"