#!/bin/bash
set -e

# Give VNC a moment to start (supervisor starts it with priority 10, this is priority 20)
echo "Waiting for VNC server to start..."
sleep 5

# Start noVNC
cd /opt/noVNC
/opt/noVNC/utils/novnc_proxy \
    --vnc localhost:${VNC_PORT:-5901} \
    --listen ${NOVNC_PORT:-6901}
