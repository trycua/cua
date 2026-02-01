#!/bin/bash
set -e

echo "Setting up Cua Codespaces environment..."

# Install Claude Code CLI
echo "Installing Claude Code CLI..."
if command -v npm &> /dev/null; then
    npm install -g @anthropic-ai/claude-code 2>/dev/null || echo "Claude Code CLI not available via npm, skipping..."
fi

# Install Cua CLI
echo "Installing Cua CLI..."
curl -fsSL https://cua.ai/install.sh | bash || echo "Cua CLI installation skipped"

# Add ccode alias for claude code
SHELL_RC="$HOME/.bashrc"
if [ -f "$HOME/.zshrc" ]; then
    SHELL_RC="$HOME/.zshrc"
fi

# Add ccode alias with OTEL configuration
if ! grep -q "alias ccode=" "$SHELL_RC" 2>/dev/null; then
    cat >> "$SHELL_RC" << 'EOF'

# Cua Codespaces aliases and OTEL configuration
alias ccode='CUA_OTEL_SERVICE_NAME=codespaces OTEL_SERVICE_NAME=codespaces claude'

# OTEL environment for Codespaces telemetry
export CUA_OTEL_ENDPOINT="${CUA_OTEL_ENDPOINT:-https://otel.cua.ai}"
export CUA_OTEL_SERVICE_NAME="${CUA_OTEL_SERVICE_NAME:-codespaces}"
export OTEL_SERVICE_NAME="${OTEL_SERVICE_NAME:-codespaces}"
export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-https://otel.cua.ai}"
export OTEL_EXPORTER_OTLP_PROTOCOL="${OTEL_EXPORTER_OTLP_PROTOCOL:-http/protobuf}"
export OTEL_RESOURCE_ATTRIBUTES="${OTEL_RESOURCE_ATTRIBUTES:-service.name=codespaces,deployment.environment=codespaces}"
EOF
    echo "Added ccode alias and OTEL configuration to $SHELL_RC"
fi

# Install Python dependencies for telemetry
if command -v pip &> /dev/null; then
    pip install opentelemetry-api opentelemetry-sdk opentelemetry-exporter-otlp-proto-http 2>/dev/null || true
fi

# Install uv for Python package management
if ! command -v uv &> /dev/null; then
    curl -LsSf https://astral.sh/uv/install.sh | sh 2>/dev/null || true
fi

echo "Cua Codespaces setup complete!"
echo "Use 'ccode' alias to run Claude Code with Codespaces OTEL telemetry enabled"
