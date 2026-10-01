#!/bin/bash

# Launch Visual Studio Code
# This script starts VS Code with a fresh window

echo "Launching Visual Studio Code..."

# Set environment variable to disable WSL detection prompt
export DONT_PROMPT_WSL_INSTALL=1

# Launch VS Code in the background
# Use /tmp as the initial workspace directory
code /tmp &

# Give it time to start and initialize
sleep 5

echo "✅ Visual Studio Code launched!"