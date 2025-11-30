#!/bin/bash
# OEM Installation Entry Point for Linux
# This script is called by the OEM systemd service on first boot

set -e

SCRIPT_DIR="/opt/oem"
LOG_FILE="$SCRIPT_DIR/setup.log"

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1" | tee -a "$LOG_FILE"
}

log "=== Starting OEM Setup ==="

# Run main setup script
if [ -f "$SCRIPT_DIR/setup.sh" ]; then
    log "Running setup.sh..."
    bash "$SCRIPT_DIR/setup.sh" 2>&1 | tee -a "$LOG_FILE"
    log "setup.sh completed with exit code: $?"
else
    log "ERROR: setup.sh not found at $SCRIPT_DIR/setup.sh"
    exit 1
fi

log "=== OEM Setup Completed ==="
