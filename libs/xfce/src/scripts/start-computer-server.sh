#!/bin/bash
set -e

# Wait for X server to be ready
echo "Waiting for X server to start..."
while ! xdpyinfo -display :1 >/dev/null 2>&1; do
    sleep 1
done
echo "X server is ready"

# Start computer-server from the venv
export DISPLAY=:1
/opt/venv/bin/python3 -m computer_server --port ${API_PORT:-8000}
