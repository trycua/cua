#!/bin/bash
set -e

# Wait for X server to be ready
echo "Waiting for X server to start..."
while ! xdpyinfo -display :1 >/dev/null 2>&1; do
    sleep 1
done
echo "X server is ready"

# Update cua-computer-server and cua-agent
echo "Updating cua-computer-server and cua-agent..."
uv pip install --upgrade --system cua-computer-server "cua-agent[all]"

# Start computer-server
export DISPLAY=:1
python3.12 -m computer_server --port ${API_PORT:-8000}
