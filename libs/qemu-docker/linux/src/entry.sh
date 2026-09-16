#!/bin/bash

cleanup() {
  echo "Received signal, shutting down gracefully..."
  if [ -n "$WATCHER_PID" ]; then
    kill "$WATCHER_PID" 2>/dev/null
  fi
  if [ -n "$VM_PID" ]; then
    kill -TERM "$VM_PID" 2>/dev/null
    wait "$VM_PID" 2>/dev/null
  fi
  exit 0
}

# Install trap for signals
trap cleanup SIGTERM SIGINT SIGHUP SIGQUIT

# Download Ubuntu ISO to /storage (persistent volume) if no disk installed yet.
# install.sh will find it there via the storage fallback path.
UBUNTU_ISO_URL="${UBUNTU_ISO_URL:-https://old-releases.ubuntu.com/releases/24.04.2/ubuntu-24.04.2-live-server-amd64.iso}"
UBUNTU_ISO_FILENAME=$(basename "$UBUNTU_ISO_URL")
UBUNTU_CHECKSUMS_URL="${UBUNTU_ISO_URL%/*}/SHA256SUMS"
STORAGE="${STORAGE:-/storage}"
if [ ! -f "$STORAGE/ubuntu.boot" ]; then
  echo "Fetching official SHA256 checksum..."
  UBUNTU_ISO_SHA256=$(curl -sL "$UBUNTU_CHECKSUMS_URL" | grep " \*${UBUNTU_ISO_FILENAME}$" | awk '{print $1}')
  if [ -z "$UBUNTU_ISO_SHA256" ]; then
    echo "ERROR: Could not fetch SHA256 for $UBUNTU_ISO_FILENAME from $UBUNTU_CHECKSUMS_URL"
    exit 1
  fi
  echo "Expected SHA256: $UBUNTU_ISO_SHA256"
  ISO_VALID=false
  if [ -f "$STORAGE/ubuntu-source.iso" ]; then
    echo "Verifying existing ISO checksum..."
    ACTUAL_SHA=$(sha256sum "$STORAGE/ubuntu-source.iso" | awk '{print $1}')
    if [ "$ACTUAL_SHA" = "$UBUNTU_ISO_SHA256" ]; then
      ISO_VALID=true
      echo "ISO checksum OK."
    else
      echo "ISO checksum mismatch (got $ACTUAL_SHA), re-downloading..."
      rm -f "$STORAGE/ubuntu-source.iso"
    fi
  fi
  if [ "$ISO_VALID" = false ]; then
    echo "Downloading Ubuntu ISO to $STORAGE/ubuntu-source.iso ..."
    aria2c -x 16 -s 16 -c -o "$STORAGE/ubuntu-source.iso" "$UBUNTU_ISO_URL" || {
      echo "ERROR: Failed to download Ubuntu ISO from $UBUNTU_ISO_URL"
      rm -f "$STORAGE/ubuntu-source.iso"
      exit 1
    }
    echo "Verifying downloaded ISO checksum..."
    ACTUAL_SHA=$(sha256sum "$STORAGE/ubuntu-source.iso" | awk '{print $1}')
    if [ "$ACTUAL_SHA" != "$UBUNTU_ISO_SHA256" ]; then
      echo "ERROR: ISO checksum mismatch after download (got $ACTUAL_SHA)"
      rm -f "$STORAGE/ubuntu-source.iso"
      exit 1
    fi
    echo "Ubuntu ISO downloaded and verified."
  fi
fi

# Start the VM in the background, tee serial output to a log file for monitoring
VM_SERIAL_LOG="/tmp/vm-serial.log"
echo "Starting Ubuntu VM..."
/usr/bin/tini -s /run/entry.sh 2>&1 | tee "$VM_SERIAL_LOG" &
VM_PID=$!
echo "Live stream accessible at localhost:8006"

# Detect first-time golden image build (no installed disk yet)
if [ ! -f "$STORAGE/ubuntu.boot" ]; then
  echo "Building golden image for the first time — this may take ~15 minutes..."
fi

echo "Waiting for Ubuntu to boot and Cua computer-server to start..."

# Background watcher: once OEM setup completes, SSH in and ensure cua-computer-server is running
(
  while true; do
    if grep -q "OEM installation completed" "$VM_SERIAL_LOG" 2>/dev/null; then
      echo "[watcher] OEM installation detected — ensuring cua-computer-server is running..."
      # Give VNC/systemd a moment to settle
      sleep 10
      VM_SSH_IP="${VM_IP:-}"
      if [ -z "$VM_SSH_IP" ]; then
        VM_SSH_IP=$(ps aux | grep dnsmasq | grep -oP '(?<=--dhcp-range=)[0-9.]+' | head -1)
      fi
      if [ -n "$VM_SSH_IP" ]; then
        sshpass -p 'cua' ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 \
          cua@"$VM_SSH_IP" \
          'sudo systemctl start cua-computer-server 2>/dev/null; sudo systemctl status cua-computer-server --no-pager' \
          && echo "[watcher] cua-computer-server started via SSH" \
          || echo "[watcher] SSH kick failed, service may self-recover via Restart=always"
      else
        echo "[watcher] Could not determine VM IP for SSH kick"
      fi
      break
    fi
    sleep 5
  done
) &
WATCHER_PID=$!

VM_IP=""
while true; do
  # Exit immediately if VM process has died
  if ! kill -0 "$VM_PID" 2>/dev/null; then
    wait "$VM_PID"
    VM_EXIT=$?
    echo "ERROR: VM process exited with code $VM_EXIT. Aborting."
    exit "$VM_EXIT"
  fi

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

# Redirect noVNC (port 8006) from QEMU framebuffer to TigerVNC inside the VM
# We use port 5701 to avoid conflicting with QEMU's built-in websocket on 5700,
# then update nginx to proxy /websockify to 5701 instead.
echo "Redirecting noVNC to TigerVNC at ${VM_IP}:5900..."
websockify 5701 "${VM_IP}:5900" &
# Remove QEMU's generated /run/shm/index.html which hardcodes a websocket
# connection to the QEMU framebuffer VNC on port 5700. Without it, nginx falls
# through to the @vnc location serving /usr/share/novnc/vnc.html, which connects
# to /websockify — our websockify proxy to TigerVNC inside the VM.
rm -f /run/shm/index.html
# The nginx config is installed from web.conf into sites-enabled/web.conf.
# Patch /websockify proxy from QEMU's port 5700 to our websockify port 5701.
sed -i 's|proxy_pass http://127.0.0.1:5700/;|proxy_pass http://127.0.0.1:5701/;|' /etc/nginx/sites-enabled/web.conf 2>/dev/null || true
nginx -s reload 2>/dev/null || true
echo "noVNC now shows TigerVNC desktop"

# Detect initial setup by presence of custom ISO
CUSTOM_ISO=$(find / -maxdepth 1 -type f -iname "*.iso" -print -quit 2>/dev/null || true)
if [ -n "$CUSTOM_ISO" ]; then
  echo "Preparation complete. Shutting down gracefully..."
  cleanup
fi

# Keep container alive for golden image boots
echo "Container running. Press Ctrl+C to stop."
tail -f /dev/null
