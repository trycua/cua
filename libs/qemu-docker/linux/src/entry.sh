#!/bin/bash

cleanup() {
  echo "Received signal, shutting down gracefully..."
  if [ -n "$VM_PID" ]; then
    kill -TERM "$VM_PID" 2>/dev/null
    wait "$VM_PID" 2>/dev/null
  fi
  exit 0
}

# Install trap for signals
trap cleanup SIGTERM SIGINT SIGHUP SIGQUIT

# Detect overlay mode: /golden exists (read-only base) but /storage is empty
# This enables running multiple isolated sessions from a single golden image
if [ -d "/golden" ] && [ ! -f "/storage/linux.boot" ]; then
    echo "Overlay mode detected, setting up copy-on-write..."
    # Try hard links first (instant, shares disk blocks until modified)
    # Fall back to regular copy if hard links fail (e.g., cross-filesystem)
    if cp -al /golden/. /storage/ 2>/dev/null; then
        echo "Overlay setup complete (using hard links)."
    else
        echo "Hard links not supported, falling back to regular copy..."
        cp -a /golden/. /storage/
        echo "Overlay setup complete (using copy)."
    fi
fi

# Start the VM in the background
echo "Starting Ubuntu VM..."
/usr/bin/tini -s /run/entry.sh &
VM_PID=$!
echo "Live stream accessible at localhost:8006"

echo "Waiting for Ubuntu to boot and Cua computer-server to start..."

VM_IP=""
while true; do
  # Wait for VM and get the IP
  if [ -z "$VM_IP" ]; then
    VM_IP=$(ps aux | grep dnsmasq | grep -oP '(?<=--dhcp-range=)[0-9.]+' | head -1)
    if [ -n "$VM_IP" ]; then
      echo "Detected VM IP: $VM_IP"
    else
      echo "Waiting for VM to start..."
      sleep 5
      continue
    fi
  fi

  # Check if server is ready
  response=$(curl --write-out '%{http_code}' --silent --output /dev/null $VM_IP:5000/status)

  if [ "${response:-0}" -eq 200 ]; then
    break
  fi

  echo "Waiting for Cua computer-server to be ready. This might take a while..."
  sleep 5
done

echo "VM is up and running, and the Cua Computer Server is ready!"

echo "Computer server accessible at localhost:5000"

# Detect initial setup by presence of custom ISO
CUSTOM_ISO=$(find / -maxdepth 1 -type f -iname "*.iso" -print -quit 2>/dev/null || true)
if [ -n "$CUSTOM_ISO" ]; then
  echo "Preparation complete. Shutting down gracefully..."
  cleanup
fi

# Keep container alive for golden image boots
echo "Container running. Press Ctrl+C to stop."
tail -f /dev/null
