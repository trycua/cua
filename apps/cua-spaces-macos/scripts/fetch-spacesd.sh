#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Prints the path of the cua-spacesd the app bundles in Contents/MacOS (its host service):
# $CUA_SPACESD_BIN when set, else the pinned release (libs/cua-spacesd/VERSION,
# the version cua-host downloads for CLI-only installs), downloaded once into
# .build/spacesd/<version>/ and verified against its published sha256.
#
#   scripts/fetch-spacesd.sh [arm64 x86_64]
#
# The release's macOS archives carry one universal binary, Developer ID
# signed by the Cua team; each requested architecture must be in it.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo="$(cd "$here/../.." && pwd)"
archs="${*:-$(uname -m)}"
if [ -n "${CUA_SPACESD_BIN:-}" ]; then
  [ -x "$CUA_SPACESD_BIN" ] || { echo "CUA_SPACESD_BIN=$CUA_SPACESD_BIN is not an executable" >&2; exit 1; }
  bin="$CUA_SPACESD_BIN"
else
  version="$(tr -d '[:space:]' < "$repo/libs/cua-spacesd/VERSION")"
  release_repo="${CUA_RELEASE_REPOSITORY:-trycua/cua}"
  dir="$here/.build/spacesd/$version"
  bin="$dir/cua-spacesd"
  if [ ! -x "$bin" ]; then
    mkdir -p "$dir"
    base="https://github.com/$release_repo/releases/download/cua-spacesd-v$version"
    asset="cua-spacesd-macos-aarch64.tar.gz"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    curl -fsSL --retry 3 -o "$tmp/$asset" "$base/$asset"
    curl -fsSL --retry 3 -o "$tmp/$asset.sha256" "$base/$asset.sha256"
    want="$(awk '{print $1}' "$tmp/$asset.sha256")"
    got="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
    [ -n "$want" ] && [ "$want" = "$got" ] || { echo "cua-spacesd $version: checksum mismatch ($got, want $want)" >&2; exit 1; }
    tar -xzf "$tmp/$asset" -C "$tmp" cua-spacesd
    install -m 0755 "$tmp/cua-spacesd" "$bin"
  fi
fi
for arch in $archs; do
  lipo "$bin" -verify_arch "$arch" || { echo "$bin has no $arch slice ($(lipo -archs "$bin"))" >&2; exit 1; }
done
echo "$bin"
