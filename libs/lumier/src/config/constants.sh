#!/usr/bin/env bash

# Port configuration
TUNNEL_PORT=8080
VNC_PORT=8006

# Host configuration
TUNNEL_HOST="host.docker.internal"

# Default VM configuration
DEFAULT_RAM_SIZE="8192"
DEFAULT_CPU_CORES="4"
DEFAULT_DISK_SIZE="100"
DEFAULT_VM_NAME="lumier"
DEFAULT_VM_VERSION="ghcr.io/trycua/macos-sequoia-vanilla:latest"

# Paths
NOVNC_PATH="/opt/noVNC"
LIFECYCLE_HOOKS_DIR="/run/hooks"

# VM connection details
HOST_USER="lume"
HOST_PASSWORD="lume"
SSH_RETRY_ATTEMPTS=20
SSH_RETRY_INTERVAL=5 