#!/bin/bash

# Launch Raneto with required configuration
INSTALL_DIR="$HOME/raneto"
cd "$INSTALL_DIR"

# Set required environment variables
export SESSION_SECRET="raneto-demo-session-secret-$(date +%s)"
export PORT="${PORT:-8080}"

# Start the server
echo "Starting Raneto server on port $PORT..."
npm start