#!/bin/bash
# Main Setup Script for Linux
# Installs dependencies and sets up CUA Computer Server

set -e

SCRIPT_DIR="/opt/oem"
LOG_FILE="$SCRIPT_DIR/setup.log"

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1" | tee -a "$LOG_FILE"
}

log "=== Running Main Setup ==="

# Update package lists
log "Updating package lists..."
sudo apt-get update

# Install Git
log "Installing Git..."
sudo apt-get install -y git

# Setup CUA Computer Server
log "Setting up CUA Computer Server..."
if [ -f "$SCRIPT_DIR/setup-cua-server.sh" ]; then
    bash "$SCRIPT_DIR/setup-cua-server.sh" 2>&1 | tee -a "$LOG_FILE"
    log "CUA Computer Server setup completed."
else
    log "ERROR: setup-cua-server.sh not found at $SCRIPT_DIR/setup-cua-server.sh"
fi

log "=== Main Setup Completed ==="
