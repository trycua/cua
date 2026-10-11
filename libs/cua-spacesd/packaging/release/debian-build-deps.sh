#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Installs the toolchain and -dev packages cua-spacesd needs into a bare
# debian:11 container (the glibc 2.31 floor of the Linux release binaries).
# Used by .github/workflows/cd-cua-spacesd.yml and build-linux-docker.sh.
#
# bullseye is past its support window and deb.debian.org is dropping it:
# the bullseye-security pool already 404s while its index still lists the
# versions the debian:11 image ships (libc6 +deb11u14), and archived main
# only has older ones, so libc6-dev cannot resolve against the installed
# libc6. The official image records the snapshot.debian.org timestamp it was
# built from (commented `# deb http://snapshot...` lines in
# /etc/apt/sources.list); when a plain install fails, apt switches to that
# snapshot, which serves exactly those versions. Snapshot Release files are
# past Valid-Until by design.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

PACKAGES=(
  git ca-certificates curl unzip xz-utils file binutils
  build-essential pkg-config perl make cmake nasm clang
  libx11-dev libx11-xcb-dev libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev
  libxrandr-dev libxi-dev libxtst-dev libxext-dev libxfixes-dev libxdamage-dev
  libwayland-dev libdbus-1-dev libudev-dev
)

install_packages() {
  apt-get "$@" update && apt-get "$@" install -y --no-install-recommends "${PACKAGES[@]}"
}

if install_packages; then
  exit 0
fi
snapshot="$(sed -n 's/^# *\(deb http:\/\/snapshot\.debian\.org\/.*\)$/\1/p' /etc/apt/sources.list)"
if [ -z "$snapshot" ]; then
  echo "apt install failed and the image records no snapshot.debian.org sources" >&2
  exit 1
fi
echo "falling back to the image's snapshot.debian.org sources:" >&2
echo "$snapshot" | sed 's/^deb /deb [check-valid-until=no] /' | tee /etc/apt/sources.list >&2
rm -f /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources
install_packages -o Acquire::Check-Valid-Until=false -o Acquire::Retries=5
