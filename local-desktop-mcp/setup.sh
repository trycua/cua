#!/bin/bash
# Setup script for Local Desktop MCP Server

set -e

echo "================================================"
echo "Local Desktop MCP Server - Setup"
echo "================================================"
echo ""

# Get the absolute path to this script's directory
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
CUA_ROOT="$(dirname "$SCRIPT_DIR")"

echo "CUA Root: $CUA_ROOT"
echo "MCP Server: $SCRIPT_DIR"
echo ""

# Check Python version
echo "Checking Python version..."
PYTHON_VERSION=$(python3 --version 2>&1 | awk '{print $2}')
PYTHON_MAJOR=$(echo $PYTHON_VERSION | cut -d. -f1)
PYTHON_MINOR=$(echo $PYTHON_VERSION | cut -d. -f2)

if [ "$PYTHON_MAJOR" -lt 3 ] || { [ "$PYTHON_MAJOR" -eq 3 ] && [ "$PYTHON_MINOR" -lt 12 ]; }; then
    echo "❌ Error: Python 3.12+ required (found: $PYTHON_VERSION)"
    echo "Please install Python 3.12 or 3.13"
    exit 1
fi

echo "✓ Python $PYTHON_VERSION"
echo ""

# Install dependencies
echo "Installing dependencies..."
echo ""

echo "→ Installing CUA core..."
pip install -e "$CUA_ROOT/libs/python/core" -q

echo "→ Installing CUA computer..."
pip install -e "$CUA_ROOT/libs/python/computer" -q

echo "→ Installing CUA computer-server..."
pip install -e "$CUA_ROOT/libs/python/computer-server" -q

echo "→ Installing MCP CLI..."
pip install 'mcp[cli]' -q

echo ""
echo "✓ Dependencies installed"
echo ""

# Detect OS
OS=$(uname -s)
echo "Detected OS: $OS"
echo ""

# OS-specific instructions
if [ "$OS" == "Linux" ]; then
    echo "📝 Linux setup notes:"
    echo "  You may need to install additional system packages:"
    echo "    sudo apt-get install python3-tk python3-dev scrot xdotool"
    echo ""
elif [ "$OS" == "Darwin" ]; then
    echo "📝 macOS setup notes:"
    echo "  You may need to grant accessibility permissions:"
    echo "  1. System Preferences → Security & Privacy → Privacy → Accessibility"
    echo "  2. Add your terminal app to the allowed apps"
    echo ""
fi

# Generate Claude Code config snippet
echo "================================================"
echo "Claude Code Configuration"
echo "================================================"
echo ""
echo "Add this to your Claude Code config file:"
echo ""

# Detect common Claude Code config locations
CLAUDE_CONFIG=""
if [ -f "$HOME/.config/claude-code/config.yaml" ]; then
    CLAUDE_CONFIG="$HOME/.config/claude-code/config.yaml"
elif [ -f "$HOME/.claude-code/config.yaml" ]; then
    CLAUDE_CONFIG="$HOME/.claude-code/config.yaml"
elif [ -f "$HOME/Library/Application Support/claude-code/config.yaml" ]; then
    CLAUDE_CONFIG="$HOME/Library/Application Support/claude-code/config.yaml"
fi

if [ -n "$CLAUDE_CONFIG" ]; then
    echo "Config file: $CLAUDE_CONFIG"
    echo ""
fi

cat << EOF
mcpServers:
  local-desktop:
    command: python3
    args:
      - $SCRIPT_DIR/server.py
    env:
      CUA_API_HOST: localhost
      CUA_API_PORT: "8000"
EOF

echo ""
echo "================================================"
echo "Next Steps"
echo "================================================"
echo ""
echo "1. Start the computer server:"
echo "   python3 -m computer_server --host localhost --port 8000"
echo ""
echo "2. In a new terminal, start Claude Code:"
echo "   claude-code"
echo ""
echo "3. Ask Claude to control your computer:"
echo "   'Take a screenshot of my desktop'"
echo ""
echo "For more information, see: $SCRIPT_DIR/README.md"
echo ""
echo "✨ Setup complete!"
