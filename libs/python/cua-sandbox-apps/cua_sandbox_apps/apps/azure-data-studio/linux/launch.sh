#!/bin/bash
# Launch script for Azure Data Studio

# Start the application in background with WSL check suppressed
export DISPLAY=:0
export DONT_PROMPT_WSL_INSTALL=1
azuredatastudio &

# Wait for the application to fully start
sleep 8

# Keep the process running
wait