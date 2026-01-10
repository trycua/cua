#!/bin/bash
# Start a simple HTTP server for the Slack clone UI
# Usage: ./start_server.sh [port]

PORT=${1:-8080}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Starting Slack clone server on http://localhost:$PORT"
echo "Press Ctrl+C to stop"

cd "$SCRIPT_DIR/gui"

# Try python3 first, fall back to python
if command -v python3 &> /dev/null; then
    python3 -m http.server "$PORT"
elif command -v python &> /dev/null; then
    python -m http.server "$PORT"
else
    echo "Error: Python is not installed"
    exit 1
fi
