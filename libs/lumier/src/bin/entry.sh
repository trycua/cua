#!/usr/bin/env bash

# Configure SSH to prevent known hosts warnings
export SSHPASS_PROMPT=
export SSH_ASKPASS=/bin/echo
# Set SSH quiet mode via the SSHPASS environment variable
export SSHPASS_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -q"

# We'll enable strict error checking AFTER initialization
# to prevent premature exits

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

# Set HOST_STORAGE_PATH to a lume ephemeral storage if not set
if [ -z "${HOST_STORAGE_PATH:-}" ]; then
    HOST_STORAGE_PATH="ephemeral"
    
    # Tell user that ephemeral storage is being used
    echo "Using ephemeral storage. VM state will be lost when macOS cleans up temporary files."
    
    export HOST_STORAGE_PATH
fi

# Only check and report mountpoints in debug mode
if [ "${LUMIER_DEBUG:-0}" == "1" ]; then
    if mountpoint -q /storage; then
        echo "/storage is mounted"
    fi
    if mountpoint -q /shared; then
        echo "/shared is mounted"
    fi
    # if mountpoint -q /data; then
    #     echo "/data is mounted"
    # fi
fi

# Check if we're running as PID 1 (important for Docker signal handling)
if [ $$ -ne 1 ]; then
    echo "Warning: This script is not running as PID 1 (current PID: $$)."
    echo "Docker signal handling may not work properly when stopped from Docker Desktop."
fi

# Log startup info
echo "Lumier VM is starting..."

# Cleanup function to ensure VM and noVNC proxy shutdown on container stop
# Counter for signal handling
SIGNAL_COUNT=0

cleanup() {
  local signal_name=$1
  set +e  # Don't exit on error in cleanup
  
  # Increment signal counter
  SIGNAL_COUNT=$((SIGNAL_COUNT + 1))
  
  # If this is the first signal, try graceful shutdown
  if [ $SIGNAL_COUNT -eq 1 ]; then
    echo "[cleanup] Caught $signal_name signal, shutting down..."
    
    # Check if we're in the middle of an image pull
    if [[ "$PULL_IN_PROGRESS" == "1" ]]; then
      echo "[cleanup] Interrupted during image pull, skipping VM stop."
    else
      echo "[cleanup] Stopping VM..."
      stop_vm true
    fi
    
    # Attempt to clean up ephemeral storage if it's in the /private/tmp directory
    if [[ "$HOST_STORAGE_PATH" == "ephemeral" ]]; then
      # First check if VM actually exists
      VM_INFO=$(lume_get "$VM_NAME" "$HOST_STORAGE_PATH" "json" "false")
      
      # Only try VM deletion if VM exists and not in the middle of a pull
      if [[ "$PULL_IN_PROGRESS" != "1" && $VM_INFO != *"Virtual machine not found"* ]]; then
        echo "[cleanup] Cleaning up VM..."
        lume_delete "$VM_NAME" "$HOST_STORAGE_PATH" > /dev/null 2>&1
      fi
    fi
  else
    # For multiple signals, force an immediate exit
    echo "got $SIGNAL_COUNT SIGTERM/SIGINTs, forcefully exiting"
  fi
  
  # If we've received multiple signals, just exit immediately
  if [ $SIGNAL_COUNT -ge 3 ]; then
    exit 1
  fi
  
  # Exit with success for the first signal
  if [ $SIGNAL_COUNT -eq 1 ]; then
    exit 0
  fi
}
# Ensure we catch all typical container termination signals
trap 'cleanup SIGTERM' SIGTERM
trap 'cleanup SIGINT' SIGINT
trap 'cleanup SIGHUP' SIGHUP

# Now enable strict error handling after initialization
set -euo pipefail

# Start the VM with error handling
if ! start_vm; then
    echo "ERROR: Failed to start VM!" >&2
    exit 1
fi

# Start noVNC for VNC access
NOVNC_PID=""
if [ -n "${VNC_PORT:-}" ] && [ -n "${VNC_PASSWORD:-}" ]; then
  # Only show this in debug mode
  if [ "${LUMIER_DEBUG:-0}" == "1" ]; then
    echo "Starting noVNC proxy with optimized color settings..."
  fi
  ${NOVNC_PATH}/utils/novnc_proxy --vnc host.docker.internal:${VNC_PORT} --listen 8006 --web ${NOVNC_PATH} > /dev/null 2>&1 &
  NOVNC_PID=$!
  disown $NOVNC_PID
  echo "noVNC interface available at: http://localhost:8006/vnc.html?password=${VNC_PASSWORD}&autoconnect=true (replace PORT with the port you forwarded to 8006)"
fi

echo "Lumier is running. Press Ctrl+C to stop."

# Instead of tail -f /dev/null, use a wait loop that can be interrupted by signals
while true; do
  # Sleep in small increments to make signal handling more responsive
  sleep 1 &
  wait $!
  # Break the loop if we've received a signal
  if [ $SIGNAL_COUNT -gt 0 ]; then
    break
  fi
done