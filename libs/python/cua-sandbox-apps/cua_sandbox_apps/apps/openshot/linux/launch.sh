#!/bin/bash
set -e

echo "Launching OpenShot Video Editor..."

# Ensure dummy audio is running (prevents "no channels" error dialog)
pulseaudio --start --exit-idle-time=-1 2>/dev/null || true
pactl load-module module-null-sink sink_name=dummy 2>/dev/null || true

# Launch OpenShot in the background and capture its PID
openshot-qt &
OPENSHOT_PID=$!

# Wait a bit for the application to start
sleep 5

# Check if process is still running
if ! kill -0 $OPENSHOT_PID 2>/dev/null; then
    echo "Warning: OpenShot process exited"
else
    echo "OpenShot launched successfully (PID: $OPENSHOT_PID)"
fi

# Keep the process running in foreground
wait $OPENSHOT_PID