#!/bin/bash
# Provision the ephemeral desktop-workspace-wayland VM for Wayland bg-input dev.
# Idempotent; re-run after any VM reset (ephemeral containerDisk loses all state).
set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y --no-install-recommends \
    build-essential meson ninja-build pkg-config libwayland-bin wayland-protocols \
    libwayland-dev libwlroots-0.19-dev libxkbcommon-dev libpixman-1-dev \
    libei-dev libeis-dev \
    wev libgtk-4-dev
echo "PROVISION_DONE"
for m in wlroots-0.19 libei-1.0 libeis-1.0; do
    printf "%s: " "$m"; pkg-config --modversion "$m" 2>/dev/null || echo MISSING
done
