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

# Start the VM in the background
echo "Starting Ubuntu VM..."
/usr/bin/tini -s /run/entry.sh &
VM_PID=$!
echo "Live stream accessible at localhost:8006"

echo "Waiting for Ubuntu to boot and cua-spacesd to start..."

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

  # Ready once cua-spacesd answers HTTP on its port (any status, e.g. 401
  # without a token, means the daemon is up).
  response=$(curl --write-out '%{http_code}' --silent --output /dev/null "http://$VM_IP:${CUA_ENV_PORT:-3211}/health")

  if [ "${response:-000}" != "000" ]; then
    break
  fi

  echo "Waiting for cua-spacesd to be ready. This might take a while..."
  sleep 5
done

echo "VM is up and running, and cua-spacesd is ready!"

echo "cua-spacesd accessible at localhost:${CUA_ENV_PORT:-3211} (token: /etc/cua/env-token in the VM)"

# Detect initial setup by presence of custom ISO
CUSTOM_ISO=$(find / -maxdepth 1 -type f -iname "*.iso" -print -quit 2>/dev/null || true)
if [ -n "$CUSTOM_ISO" ]; then
  echo "Preparation complete. Shutting down gracefully..."
  cleanup
fi

# Keep container alive for golden image boots
echo "Container running. Press Ctrl+C to stop."
tail -f /dev/null
