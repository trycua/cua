#!/bin/bash
# Main Setup Script for Linux
# Installs dependencies and sets up cua-spacesd

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

# Setup cua-spacesd
log "Setting up cua-spacesd..."
if [ -f "$SCRIPT_DIR/setup-spacesd.sh" ]; then
    bash "$SCRIPT_DIR/setup-spacesd.sh" 2>&1 | tee -a "$LOG_FILE"
    log "cua-spacesd setup completed."
else
    log "ERROR: setup-spacesd.sh not found at $SCRIPT_DIR/setup-spacesd.sh"
fi

log "=== Main Setup Completed ==="
