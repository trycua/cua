#!/bin/bash

echo "=== Launching Eclipse IDE ==="

# Determine installation directory
INSTALL_DIR="$HOME/.local/opt/eclipse-ide"
ECLIPSE_BIN="$INSTALL_DIR/eclipse/eclipse"

if [ ! -f "$ECLIPSE_BIN" ]; then
    echo "ERROR: Eclipse binary not found at $ECLIPSE_BIN"
    exit 1
fi

# Set required environment variables
export JAVA_HOME=${JAVA_HOME:-$(which java | xargs dirname | xargs dirname)}
export DISPLAY=:1

# Launch Eclipse in the background
echo "Launching Eclipse from: $ECLIPSE_BIN"
"$ECLIPSE_BIN" -data "$HOME/.eclipse/workspace" &

# Get the PID of the launched process
ECLIPSE_PID=$!
echo "Eclipse PID: $ECLIPSE_PID"

# Wait a bit for the application to start
sleep 5

echo "✓ Eclipse launched successfully"