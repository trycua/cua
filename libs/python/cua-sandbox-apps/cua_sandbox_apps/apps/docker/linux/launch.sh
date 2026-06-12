#!/bin/bash
set -e

# Kill any existing dockerd processes
sudo pkill -9 dockerd || true
sleep 1

# Start Docker daemon with simplified options for sandbox environment
echo "Starting Docker daemon..."
sudo nohup dockerd --iptables=false --ip-forward=false --bridge=none > /tmp/dockerd.log 2>&1 &
DAEMON_PID=$!
echo "Docker daemon PID: $DAEMON_PID"

# Wait for Docker socket to be created
echo "Waiting for Docker socket..."
for i in {1..30}; do
    if [ -S /var/run/docker.sock ]; then
        echo "✓ Docker socket found"
        break
    fi
    if [ $i -eq 30 ]; then
        echo "✗ Docker daemon socket not found"
        tail -20 /tmp/dockerd.log 2>/dev/null || true
        exit 1
    fi
    sleep 1
done

# Wait for daemon to respond
echo "Waiting for Docker daemon to be responsive..."
for i in {1..30}; do
    if sudo docker ps &>/dev/null; then
        echo "✓ Docker daemon is responsive"
        break
    fi
    if [ $i -eq 30 ]; then
        echo "✗ Docker daemon not responding"
        tail -30 /tmp/dockerd.log 2>/dev/null || true
        exit 1
    fi
    sleep 1
done

# Display Docker info
echo ""
echo "=== Docker Version ==="
sudo docker --version
echo ""
echo "=== Docker System Info ==="
sudo docker info
echo ""
echo "✓ Docker is running successfully"
