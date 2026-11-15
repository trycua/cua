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

# Shared retry policy: per-fetch retries and timeouts (also see the inline
# bootstraps in cd-rust-cua-driver.yml and cd-cua-sdk.yml).
mkdir -p /etc/apt/apt.conf.d
cat > /etc/apt/apt.conf.d/80-cua-retries <<'EOF'
Acquire::Retries "5";
Acquire::http::Timeout "30";
Acquire::https::Timeout "30";
EOF

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
  echo "apt mirror used: image default (deb.debian.org)" >&2
  exit 0
fi
echo "::warning::default mirror failed after retries; falling back to archive.debian.org" >&2
cat > /etc/apt/sources.list <<'EOF'
deb [check-valid-until=no] http://archive.debian.org/debian bullseye main
deb [check-valid-until=no] http://archive.debian.org/debian bullseye-updates main
deb [check-valid-until=no] http://archive.debian.org/debian-security bullseye-security main
EOF
rm -f /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources
install_packages -o Acquire::Check-Valid-Until=false -o Acquire::Retries=5
echo "apt mirror used: archive.debian.org (fallback)" >&2
