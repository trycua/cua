#!/bin/bash

# Launch Godot Engine
# The --editor flag starts the editor interface

export DISPLAY=:1
# Use software rendering (llvmpipe/swrast) in containers without GPU access.
# LIBGL_ALWAYS_SOFTWARE forces Mesa's software rasteriser even when a GPU DRI
# device is present but inaccessible — avoids "libGL: screen 0 does not appear
# to be DRI2/DRI3 capable" and similar errors in headless/containerised envs.
export LIBGL_ALWAYS_SOFTWARE=1

/opt/godot/godot --editor > /tmp/godot.log 2>&1 &
GODOT_PID=$!

echo "Godot Engine launched with PID: $GODOT_PID"

# Wait for the window to appear
sleep 8

exit 0
