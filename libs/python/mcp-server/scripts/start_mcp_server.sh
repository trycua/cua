#!/bin/bash

set -e

# Set the CUA repository path based on script location
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"
CUA_REPO_DIR="$( cd "$SCRIPT_DIR/../../.." &> /dev/null && pwd )"
PYTHON_PATH="${CUA_REPO_DIR}/.venv/bin/python"

# Set Python path to include all necessary libraries
export PYTHONPATH="${CUA_REPO_DIR}/libs/python/mcp-server:${CUA_REPO_DIR}/libs/python/agent:${CUA_REPO_DIR}/libs/python/computer:${CUA_REPO_DIR}/libs/python/core"

# Run the MCP server directly as a module
$PYTHON_PATH -m mcp_server.server