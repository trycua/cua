#!/bin/bash
#
# CUA Windows Entry Script
#
# This script:
# 1. Starts the Windows VM in background
# 2. Waits for cua-computer-server to be ready
# 3. Keeps container running
#
# The client (cua-bench) is responsible for:
# - Polling the /status endpoint
# - Sending commands via the /cmd endpoint

set -e

# VM network configuration
VM_IP="${VM_IP:-172.30.0.2}"
SERVER_PORT="${SERVER_PORT:-5000}"
BOOT_TIMEOUT="${BOOT_TIMEOUT:-300}"  # 5 minutes max

echo "============================================"
echo "CUA Windows Container"
echo "============================================"
echo "VM IP: ${VM_IP}"
echo "Server Port: ${SERVER_PORT}"
echo "Install WinArena Apps: ${INSTALL_WINARENA_APPS:-false}"
echo ""

# Signal handling for graceful shutdown
cleanup() {
    echo "Shutting down..."
    # The base image handles QEMU shutdown
    exit 0
}
trap cleanup SIGTERM SIGINT

# Start the VM using the base image's entry script
echo "Starting Windows VM..."
/usr/bin/tini -s /run/entry.sh &
VM_PID=$!

# Wait for cua-computer-server to be ready
echo "Waiting for cua-computer-server at ${VM_IP}:${SERVER_PORT}..."
start_time=$(date +%s)

while true; do
    current_time=$(date +%s)
    elapsed=$((current_time - start_time))

    if [ $elapsed -ge $BOOT_TIMEOUT ]; then
        echo "ERROR: Boot timeout after ${BOOT_TIMEOUT}s"
        exit 1
    fi

    # Check if server is responding
    response=$(curl --write-out '%{http_code}' --silent --output /dev/null --max-time 5 "http://${VM_IP}:${SERVER_PORT}/status" 2>/dev/null || echo "000")

    if [ "$response" = "200" ]; then
        echo ""
        echo "============================================"
        echo "Windows VM is ready!"
        echo "  API: http://localhost:${SERVER_PORT}"
        echo "  VNC: http://localhost:8006"
        echo "============================================"
        break
    fi

    echo "  Waiting... (${elapsed}s)"
    sleep 5
done

# Keep container running
echo "Container running. Press Ctrl+C to stop."
wait $VM_PID
