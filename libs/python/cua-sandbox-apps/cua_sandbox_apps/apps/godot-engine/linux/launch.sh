#!/bin/bash

# Launch Godot Engine
# The --editor flag starts the editor interface

export DISPLAY=:1
export LIBGL_ALWAYS_INDIRECT=1

/opt/godot/godot --editor > /tmp/godot.log 2>&1 &
GODOT_PID=$!

echo "Godot Engine launched with PID: $GODOT_PID"

# Wait for the window to appear
sleep 8

exit 0