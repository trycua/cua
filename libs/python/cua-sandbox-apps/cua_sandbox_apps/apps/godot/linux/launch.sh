#!/bin/bash
set -e

# Set display for headless systems
export DISPLAY=:99

# Kill any existing processes from previous runs
pkill Xvfb 2>/dev/null || true
pkill openbox 2>/dev/null || true
sleep 1

# Start Xvfb display server
Xvfb :99 -screen 0 1024x768x24 > /tmp/xvfb.log 2>&1 &
XVFB_PID=$!

# Wait for Xvfb to initialize
sleep 2

# Start window manager
openbox > /tmp/openbox.log 2>&1 &
WM_PID=$!

# Wait for window manager to start
sleep 1

# Launch Godot Engine GUI
/usr/local/bin/godot "$@" &
GODOT_PID=$!

# Wait for app to fully render
sleep 8

# Application is now running
echo "Godot Engine is now running with PID $GODOT_PID"

# Keep the script running while Godot runs
wait $GODOT_PID 2>/dev/null || true

# Cleanup
kill $WM_PID 2>/dev/null || true
kill $XVFB_PID 2>/dev/null || true