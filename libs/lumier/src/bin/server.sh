#!/usr/bin/env bash

# Exit on errors, undefined variables, and propagate errors in pipes
set -euo pipefail

# Source constants if available
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -f "${SCRIPT_DIR}/../config/constants.sh" ]; then
  source "${SCRIPT_DIR}/../config/constants.sh"
fi

# Use the tunnel port from constants if available, otherwise default to 8080
PORT="${TUNNEL_PORT:-8080}"
TUNNEL_SCRIPT="${SCRIPT_DIR}/tunnel.sh"

# Function to check if the tunnel is active
is_tunnel_active() {
    if lsof -i TCP:$PORT 2>/dev/null | grep LISTEN > /dev/null; then
        return 0  # Tunnel is active
    else
        return 1  # Tunnel is not active
    fi
}

# Function to start the tunnel
start_tunnel() {
    echo "Starting tunnel on port $PORT..."
    if is_tunnel_active; then
        echo "Tunnel is already running on port $PORT."
        return 0
    fi
    
    # Start socat in the background
    socat TCP-LISTEN:$PORT,reuseaddr,fork EXEC:"$TUNNEL_SCRIPT" &
    SOCAT_PID=$!
    
    # Check if the tunnel started successfully
    sleep 1
    if ! is_tunnel_active; then
        echo "Failed to start tunnel on port $PORT."
        return 1
    fi
    
    echo "Tunnel started successfully on port $PORT (PID: $SOCAT_PID)."
    return 0
}

# Function to stop the tunnel
stop_tunnel() {
    echo "Stopping tunnel on port $PORT..."
    if ! is_tunnel_active; then
        echo "No tunnel running on port $PORT."
        return 0
    fi
    
    # Find and kill the socat process
    local pid=$(lsof -i TCP:$PORT | grep LISTEN | awk '{print $2}')
    if [ -n "$pid" ]; then
        kill $pid
        echo "Tunnel stopped (PID: $pid)."
        return 0
    else
        echo "Failed to find process using port $PORT."
        return 1
    fi
}

# Function to check tunnel status
status_tunnel() {
    if is_tunnel_active; then
        local pid=$(lsof -i TCP:$PORT | grep LISTEN | awk '{print $2}')
        echo "Tunnel is active on port $PORT (PID: $pid)."
        return 0
    else
        echo "No tunnel running on port $PORT."
        return 1
    fi
}

# Parse command line arguments
case "${1:-}" in
    start)
        start_tunnel
        ;;
    stop)
        stop_tunnel
        ;;
    restart)
        stop_tunnel
        start_tunnel
        ;;
    status)
        status_tunnel
        ;;
    *)
        echo "Usage: $0 {start|stop|restart|status}"
        exit 1
        ;;
esac 