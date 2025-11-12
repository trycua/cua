#!/bin/bash

# Create windows.boot file if it doesn't exist (required for proper boot)
if [ -d "/storage" -a ! -f "/storage/windows.boot" ]; then
  echo "Creating windows.boot file in /storage..."
  touch /storage/windows.boot
fi

# Start the VM in the background
echo "Starting Windows VM..."
/usr/bin/tini -s /run/entry.sh &
echo "Live stream accessible at localhost:8006"

echo "Waiting for Windows to boot and CUA computer-server to start..."

VM_IP=""
while true; do
  # Wait from VM and get the IP
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

  echo "Waiting for CUA computer-server to be ready. This might take a while..."
  sleep 5
done

echo "VM is up and running, and the CUA Computer Server is ready!"

echo "Computer server accessible at localhost:5000"

# Detect initial setup by presence of /custom.iso (setup ISO mount)
if [ ! -f "/custom.iso" ]; then # Keep container alive
  echo "Container running. Press Ctrl+C to stop."
  tail -f /dev/null
fi
