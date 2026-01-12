#!/bin/bash

echo "Starting WinArena VM..."

# Start the VM script in the background
cd /
./start_vm.sh &

# Get VM IP from DHCP lease or use default
# The VM gets its IP via DHCP from dnsmasq on the docker bridge
VM_IP="${VM_IP:-172.30.0.2}"
SERVER_PORT="${SERVER_PORT:-5000}"

echo "Waiting for CUA Computer Server at ${VM_IP}:${SERVER_PORT}..."

# Wait for the VM to start up
while true; do
  # Send a GET request to the /status endpoint (cua-computer-server health check)
  response=$(curl --write-out '%{http_code}' --silent --output /dev/null ${VM_IP}:${SERVER_PORT}/status)

  # If the response code is 200 (HTTP OK), break the loop
  if [ $response -eq 200 ]; then
    break
  fi

  echo "Waiting for a response from the CUA Computer Server. This might take a while..."

  # Wait for a while before the next attempt
  sleep 5
done

echo "VM is up and running, and the CUA Computer Server is ready to use!"