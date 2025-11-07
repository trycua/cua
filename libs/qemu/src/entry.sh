#!/bin/bash

# Fix for Azure ML Job not using the correct root path
cd /

# Start the VM in the background
echo "Starting Windows VM..."
/usr/bin/tini -s /run/entry.sh &
echo "Live stream accessible at localhost:8006"

# Wait for the VM to start up and CUA computer-server to be ready
echo "Waiting for Windows to boot and CUA computer-server to start..."
while true; do
  # Send a GET request to check if server is ready
  response=$(curl --write-out '%{http_code}' --silent --output /dev/null 20.20.20.21:5000/status)

  # If the response code is 200 (HTTP OK), break the loop
  if [ "${response:-0}" -eq 200 ]; then
    break
  fi

  echo "Waiting for CUA computer-server to be ready. This might take a while..."

  # Wait for a while before the next attempt
  sleep 5
done

echo "VM is up and running, and the CUA Computer Server is ready!"

# Set up port forwarding from localhost:5000 to VM emulator IP
echo "Setting up port forwarding: localhost:5000 -> 20.20.20.21:5000"
socat TCP-LISTEN:5000,fork,reuseaddr TCP:20.20.20.21:5000 &

echo "Computer server accessible at localhost:5000"

# Keep container alive
echo "Container running. Press Ctrl+C to stop."
tail -f /dev/null
