#!/bin/bash

echo "=== GitLab Launch Script ==="
echo ""

# Check if GitLab is installed
if [ ! -f /opt/gitlab/bin/gitlab-ctl ]; then
    echo "✗ GitLab is not installed."
    echo "Please run: install_gitlab.sh first"
    exit 1
fi

echo "Starting GitLab services..."
echo ""

# Run reconfigure to set up and start all services
echo "Configuring and starting GitLab (this may take a few minutes)..."
sudo /opt/gitlab/bin/gitlab-ctl reconfigure 2>&1 | tail -30

# Wait for services to stabilize
echo ""
echo "Waiting for services to start..."
sleep 10

# Check status
echo ""
echo "Service Status:"
sudo /opt/gitlab/bin/gitlab-ctl status 2>&1 || true

echo ""
echo "=== GitLab Starting ==="
echo "Access GitLab at: http://localhost:8080"
echo ""
echo "Initial Setup:"
echo "  - Username: root"
echo "  - Password: see /etc/gitlab/initial_root_password"
echo ""
echo "Useful commands:"
echo "  sudo /opt/gitlab/bin/gitlab-ctl status      - Check service status"
echo "  sudo /opt/gitlab/bin/gitlab-ctl tail         - View live logs"
echo "  sudo /opt/gitlab/bin/gitlab-ctl restart      - Restart services"
echo "  sudo /opt/gitlab/bin/gitlab-ctl reconfigure  - Reconfigure all services"
echo ""
