#!/usr/bin/env bash
# Test script to run the MCP server and see all errors
# This is for LOCAL DEVELOPMENT ONLY - helps debug issues before connecting from Claude

set -e  # Exit on error, but we'll see the error

echo "=========================================="
echo "CUA MCP Server - Development Test"
echo "=========================================="
echo ""

# Get the repo root
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"
CUA_REPO_DIR="$( cd "$SCRIPT_DIR/../../../.." &> /dev/null && pwd )"

echo "Repo directory: $CUA_REPO_DIR"

# Find Python
if [ -f "$CUA_REPO_DIR/.venv/bin/python" ]; then
    PYTHON="$CUA_REPO_DIR/.venv/bin/python"
elif [ -f "$CUA_REPO_DIR/libs/.venv/bin/python" ]; then
    PYTHON="$CUA_REPO_DIR/libs/.venv/bin/python"
else
    PYTHON="python3"
fi

echo "Using Python: $PYTHON"
$PYTHON --version
echo ""

# Set up environment
export PYTHONPATH="$CUA_REPO_DIR/libs/python/mcp-server:$CUA_REPO_DIR/libs/python/agent:$CUA_REPO_DIR/libs/python/computer:$CUA_REPO_DIR/libs/python/core"
export CUA_MODEL_NAME="${CUA_MODEL_NAME:-anthropic/claude-sonnet-4-5-20250929}"

echo "Environment:"
echo "  PYTHONPATH: $PYTHONPATH"
echo "  CUA_MODEL_NAME: $CUA_MODEL_NAME"
echo ""
echo "=========================================="
echo "Starting MCP server..."
echo "Press Ctrl+C to stop"
echo "=========================================="
echo ""

# Run the server WITHOUT redirecting stderr, so we can see all errors
exec "$PYTHON" -m mcp_server.server
