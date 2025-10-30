#!/bin/bash
set -e

# Start D-Bus
if [ -z "$DBUS_SESSION_BUS_ADDRESS" ]; then
    eval $(dbus-launch --sh-syntax --exit-with-session)
fi

# Start XFCE
startxfce4 &

# Wait for XFCE to start
sleep 2

# Disable screensaver and power management
xset s off
xset -dpms
xset s noblank

# Wait for the session
wait
