#!/usr/bin/env bash

# Exit on errors, undefined variables, and propagate errors in pipes
set -euo pipefail

# Source configuration files
CONFIG_DIR="/run/config"
LIB_DIR="/run/lib"

# Source constants if available
if [ -f "${CONFIG_DIR}/constants.sh" ]; then
  source "${CONFIG_DIR}/constants.sh"
fi

# Import utilities
for lib in "${LIB_DIR}"/*.sh; do
  if [ -f "$lib" ]; then
    source "$lib"
  fi
done

# Set VM_NAME to env or fallback to container name (from --name)
if [ -z "${VM_NAME:-}" ]; then
    VM_NAME="$(cat /etc/hostname)"
    export VM_NAME
fi

# Set HOST_STORAGE_PATH to /storage/$VM_NAME if not set
if [ -z "${HOST_STORAGE_PATH:-}" ]; then
    HOST_STORAGE_PATH="/storage/$VM_NAME"
    export HOST_STORAGE_PATH
fi

# Optionally check for mountpoints
if mountpoint -q /storage; then
    echo "/storage is mounted"
fi
if mountpoint -q /data; then
    echo "/data is mounted"
fi

# Log startup info
echo "Lumier VM is starting..."

# Cleanup function to ensure VM and noVNC proxy shutdown on container stop
cleanup() {
  set +e  # Don't exit on error in cleanup
  echo "[cleanup] Caught signal, shutting down..."
  echo "[cleanup] Stopping VM..."
  stop_vm
  # Now gently stop noVNC proxy if running
  # if [ -n "${NOVNC_PID:-}" ] && kill -0 "$NOVNC_PID" 2>/dev/null; then
  #   echo "[cleanup] Stopping noVNC proxy (PID $NOVNC_PID)..."
  #   kill -TERM "$NOVNC_PID"
  #   # Wait up to 5s for noVNC to exit
  #   for i in {1..5}; do
  #     if ! kill -0 "$NOVNC_PID" 2>/dev/null; then
  #       echo "[cleanup] noVNC proxy stopped."
  #       break
  #     fi
  #     sleep 1
  #   done
  #   # Escalate if still running
  #   if kill -0 "$NOVNC_PID" 2>/dev/null; then
  #     echo "[cleanup] noVNC proxy did not exit, killing..."
  #     kill -KILL "$NOVNC_PID" 2>/dev/null
  #   fi
  # fi
  echo "[cleanup] Done. Exiting."
  exit 0
}
trap cleanup SIGTERM SIGINT

# Start the VM
start_vm

# Start noVNC for VNC access
NOVNC_PID=""
if [ -n "${VNC_PORT:-}" ] && [ -n "${VNC_PASSWORD:-}" ]; then
  echo "Starting noVNC proxy with optimized color settings..."
  ${NOVNC_PATH}/utils/novnc_proxy --vnc host.docker.internal:${VNC_PORT} --listen 8006 --web ${NOVNC_PATH} > /dev/null 2>&1 &
  NOVNC_PID=$!
  disown $NOVNC_PID
  echo "noVNC interface available at: http://localhost:8006/vnc.html?password=${VNC_PASSWORD}&autoconnect=true&logging=debug"
fi

# Run any post-startup hooks
if [ -d "/run/hooks" ]; then
  for hook in /run/hooks/*; do
    if [ -x "$hook" ]; then
      echo "Running hook: $(basename "$hook")"
      "$hook"
    fi
  done
fi

echo "Lumier is running. Press Ctrl+C to stop."
tail -f /dev/null