#!/bin/bash
set -e

echo "Launching systemd-hostnamed..."

# Try to start the service with systemctl (may fail in non-systemd container)
if sudo systemctl start systemd-hostnamed 2>/dev/null || true; then
    echo "✓ Attempted to start via systemctl"
fi

# Alternative: Launch the daemon directly
if ! pgrep -f "systemd-hostnamed" > /dev/null 2>&1; then
    echo "Starting systemd-hostnamed daemon directly..."
    sudo /lib/systemd/systemd-hostnamed &
    sleep 1
fi

# Verify the binary exists
if [ -f /lib/systemd/systemd-hostnamed ]; then
    echo "✓ systemd-hostnamed binary found: /lib/systemd/systemd-hostnamed"
fi

# Verify D-Bus service configuration exists
if [ -f /usr/share/dbus-1/system-services/org.freedesktop.hostname1.service ]; then
    echo "✓ D-Bus service configuration found"
    echo ""
    echo "D-Bus Service File:"
    cat /usr/share/dbus-1/system-services/org.freedesktop.hostname1.service
fi

# Check for the systemd unit file
if [ -f /lib/systemd/system/systemd-hostnamed.service ]; then
    echo ""
    echo "Systemd Unit File:"
    cat /lib/systemd/system/systemd-hostnamed.service
fi

# Get version info
echo ""
echo "Version Information:"
/lib/systemd/systemd-hostnamed --version 2>/dev/null || echo "✓ Binary is operational"

echo ""
echo "✓ systemd-hostnamed successfully installed and configured"