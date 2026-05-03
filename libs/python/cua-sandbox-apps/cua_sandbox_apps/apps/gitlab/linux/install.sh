#!/bin/bash
set -e

echo "=== GitLab Community Edition Installation Script ==="
echo "Target: Ubuntu/Debian Linux Systems"
echo ""

# Check if already installed
if [ -f /opt/gitlab/bin/gitlab-ctl ]; then
    echo "✓ GitLab is already installed at /opt/gitlab"
    exit 0
fi

echo "Step 1: Updating system packages..."
sudo apt-get update

echo "Step 2: Installing prerequisites..."
sudo apt-get install -y \
    curl \
    ca-certificates \
    openssh-server \
    openssh-client \
    perl

echo "Step 3: Adding GitLab Community Edition repository..."
curl https://packages.gitlab.com/install/repositories/gitlab/gitlab-ce/script.deb.sh | sudo bash

echo "Step 4: Installing GitLab Community Edition..."
echo "This will download and install ~1.4GB of packages and may take 3-10 minutes..."
sudo DEBIAN_FRONTEND=noninteractive \
    EXTERNAL_URL="${EXTERNAL_URL:-http://localhost:8080}" \
    apt-get install -y gitlab-ce

echo ""
echo "✓ GitLab Community Edition has been installed successfully!"
echo ""
echo "Location: /opt/gitlab"
echo "Binary: /opt/gitlab/bin/gitlab-ctl"
echo "Configuration: /etc/gitlab/gitlab.rb"
echo ""
echo "Next steps:"
echo "  1. Run: sudo /opt/gitlab/bin/gitlab-ctl reconfigure"
echo "  2. Wait for services to start (may take 2-3 minutes)"
echo "  3. Access GitLab at: http://localhost:8080"
echo "  4. Default username: root"
echo "  5. Root password: see /etc/gitlab/initial_root_password"
echo ""
