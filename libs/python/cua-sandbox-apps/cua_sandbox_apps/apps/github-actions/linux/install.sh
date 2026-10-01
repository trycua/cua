#!/bin/bash
set -e

echo "=== Installing GitHub Actions Runner ==="

# Install dependencies with sudo if needed
echo "Installing dependencies..."
if [ "$EUID" -ne 0 ]; then 
    sudo apt-get update -qq > /dev/null 2>&1
    sudo apt-get install -y -qq \
        curl \
        tar \
        git \
        wget \
        ca-certificates > /dev/null 2>&1
    SUDO="sudo"
else
    apt-get update -qq > /dev/null 2>&1
    apt-get install -y -qq \
        curl \
        tar \
        git \
        wget \
        ca-certificates > /dev/null 2>&1
    SUDO=""
fi

# Create directory for runner - use home directory
RUNNER_DIR="$HOME/.github-actions-runner"
mkdir -p "$RUNNER_DIR"
cd "$RUNNER_DIR"

# Download latest runner
echo "Downloading GitHub Actions Runner (v2.333.1)..."
RUNNER_VERSION="2.333.1"
RUNNER_FILE="actions-runner-linux-x64-${RUNNER_VERSION}.tar.gz"
DOWNLOAD_URL="https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/${RUNNER_FILE}"

# Use timeout and retry logic
max_attempts=3
attempt=1
while [ $attempt -le $max_attempts ]; do
    echo "Attempt $attempt/$max_attempts..."
    if curl -fsSL --max-time 30 -o "$RUNNER_FILE" "$DOWNLOAD_URL"; then
        break
    fi
    attempt=$((attempt + 1))
    sleep 2
done

if [ ! -f "$RUNNER_FILE" ]; then
    echo "Error: Failed to download runner after $max_attempts attempts"
    exit 1
fi

# Extract runner
echo "Extracting runner..."
tar -xzf "$RUNNER_FILE"
rm "$RUNNER_FILE"

# Make scripts executable
chmod +x "$RUNNER_DIR/run.sh" || true
chmod +x "$RUNNER_DIR/config.sh" || true

# Create symlinks if possible
if [ "$EUID" -eq 0 ]; then
    ln -sf "$RUNNER_DIR/run.sh" /usr/local/bin/github-actions-runner || true
    ln -sf "$RUNNER_DIR/config.sh" /usr/local/bin/github-actions-runner-config || true
else
    # Try with sudo, but don't fail if it doesn't work
    sudo ln -sf "$RUNNER_DIR/run.sh" /usr/local/bin/github-actions-runner 2>/dev/null || true
    sudo ln -sf "$RUNNER_DIR/config.sh" /usr/local/bin/github-actions-runner-config 2>/dev/null || true
fi

# Display runner info
echo ""
echo "=== Installation Complete ==="
echo "Runner installed at: $RUNNER_DIR"
echo ""
echo "Installation successful!"