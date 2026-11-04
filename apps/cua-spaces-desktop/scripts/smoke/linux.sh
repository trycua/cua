#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Linux smoke test for the packaged app, in Docker, under Xvfb.
#
#   scripts/smoke/linux.sh [arm64|x64]     (default: the host's arch)
#
# For each arch it:
#   1. installs the .deb and checks the desktop entry, icons and WM class;
#   2. runs the AppImage (extract-and-run, no FUSE) under Xvfb + openbox,
#      grabs a full-screen screenshot and the window's WM_CLASS;
#   3. runs the AppImage again with CUA_SPACES_CAPTURE_ROUTES, which loads
#      every route in light and dark and reports the bridge mode per page.
# Output goes to dist/smoke/linux-<arch>/. Nothing leaves the machine.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD
version=$(node -p 'require("./package.json").version')
host=$(uname -m | sed 's/aarch64/arm64/;s/x86_64/x64/')
archs=("${@:-$host}")
image=cua-spaces-smoke:ubuntu24

for arch in "${archs[@]}"; do
  case $arch in
    arm64) platform=linux/arm64; appimage="Cua-Spaces-$version-arm64.AppImage"; deb="cua-spaces_${version}_arm64.deb" ;;
    x64) platform=linux/amd64; appimage="Cua-Spaces-$version-x86_64.AppImage"; deb="cua-spaces_${version}_amd64.deb" ;;
    *) echo "unknown arch $arch" >&2; exit 2 ;;
  esac
  out="$root/dist/smoke/linux-$arch"
  rm -rf "$out" && mkdir -p "$out"
  docker build -q --platform "$platform" -t "$image-$arch" -f scripts/smoke/linux.Dockerfile scripts/smoke >/dev/null
  docker run --rm --platform "$platform" --shm-size=1g \
    -e APPIMAGE="/dist/$appimage" -e DEB="/dist/$deb" -e EMULATED=$([ "$arch" = "$host" ] && echo 0 || echo 1) \
    -v "$root/dist:/dist:ro" -v "$out:/out" -v "$root/scripts/smoke/linux-inside.sh:/smoke.sh:ro" \
    "$image-$arch" bash /smoke.sh 2>&1 | tee "$out/log.txt"
done
