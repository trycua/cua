#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Installs the toolchain and -dev packages cua-spacesd needs into a bare
# debian:11 container (the glibc 2.31 floor of the Linux release binaries).
# Used by .github/workflows/cd-cua-spacesd.yml and build-linux-docker.sh.
#
# bullseye is past its support window and deb.debian.org is dropping it:
# the bullseye-security pool already 404s while its index still lists the
# versions the debian:11 image ships (libc6 +deb11u14). archive.debian.org
# keeps those versions (its debian-security tree has libc6-dev +deb11u14),
# so a plain install that fails switches to archive.debian.org and retries.
# snapshot.debian.org is not used: its outages broke two release runs on
# 5 Nov 2026. Archive Release files are past Valid-Until by design.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

PACKAGES=(
  git ca-certificates curl unzip xz-utils file binutils
  build-essential pkg-config perl make cmake nasm clang
  libx11-dev libx11-xcb-dev libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev
  libxrandr-dev libxi-dev libxtst-dev libxext-dev libxfixes-dev libxdamage-dev
  libwayland-dev libdbus-1-dev libudev-dev
  musl-tools
)

install_packages() {
  apt-get -o Acquire::Retries=5 "$@" update && apt-get -o Acquire::Retries=5 "$@" install -y --no-install-recommends "${PACKAGES[@]}"
}

if install_packages; then
  exit 0
fi
echo "falling back to archive.debian.org sources" >&2
cat > /etc/apt/sources.list <<'EOF'
deb [check-valid-until=no] http://archive.debian.org/debian bullseye main
deb [check-valid-until=no] http://archive.debian.org/debian bullseye-updates main
deb [check-valid-until=no] http://archive.debian.org/debian-security bullseye-security main
EOF
rm -f /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources
install_packages -o Acquire::Check-Valid-Until=false -o Acquire::Retries=5
