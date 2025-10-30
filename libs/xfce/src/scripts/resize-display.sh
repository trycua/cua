#!/bin/bash
# Dynamic display resolution script
# Can be called to change the VNC display resolution

RESOLUTION=${1:-1920x1080}

# Wait for display to be ready
for i in {1..10}; do
    if DISPLAY=:1 xdpyinfo >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

# Change resolution using xrandr
DISPLAY=:1 xrandr --output VNC-0 --mode "$RESOLUTION" 2>/dev/null || \
DISPLAY=:1 xrandr --fb "$RESOLUTION" 2>/dev/null || \
echo "Failed to set resolution to $RESOLUTION"

echo "Display resolution set to: $RESOLUTION"
