#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Prints the folder holding the Cua Driver the app bundles in
# Contents/Helpers (cua-driver and cua-cursor-theme): $CUA_DRIVER_DIR when
# set, else the release pinned in cua-driver.release.json, downloaded once
# into .build/cua-driver/<version>/.
#
#   scripts/fetch-cua-driver.sh [arm64 x86_64]
#
# The download is checked against the size and SHA-256 committed in
# cua-driver.release.json, never against a checksum file from the release
# itself: whoever can replace a release asset can replace its .sha256 too.
# Each file must also be Developer ID signed by the team the table names,
# with the hardened runtime, and carry every requested architecture.
#
# To move to a new driver release, change version, tag, asset, bytes and
# sha256 together, copying bytes and sha256 from the release's assets.
#
# For tests: CUA_DRIVER_RELEASE_TABLE (another table) and
# CUA_DRIVER_RELEASE_BASE_URL (where the asset is, instead of the GitHub
# release download URL).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
archs="${*:-$(uname -m)}"
table="${CUA_DRIVER_RELEASE_TABLE:-$here/cua-driver.release.json}"

field() {
  plutil -extract "$1" raw -o - "$table" 2>/dev/null ||
    { echo "$table has no $1" >&2; exit 1; }
}
version="$(field version)"
tag="$(field tag)"
asset="$(field asset)"
bytes="$(field bytes)"
sha256="$(field sha256)"
team="$(field teamId)"
files=()
n="$(plutil -extract files raw -o - "$table")"
for ((i = 0; i < n; i++)); do files+=("$(field "files.$i")"); done
[ "${#files[@]}" -gt 0 ] || { echo "$table lists no files" >&2; exit 1; }

if [ -n "${CUA_DRIVER_DIR:-}" ]; then
  dir="$CUA_DRIVER_DIR"
else
  dir="$here/.build/cua-driver/$version"
  if [ ! -f "$dir/.verified" ]; then
    rm -rf "$dir"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    base="${CUA_DRIVER_RELEASE_BASE_URL:-https://github.com/trycua/cua/releases/download/$tag}"
    curl -fsSL --retry 3 -o "$tmp/$asset" "$base/$asset"
    got_bytes="$(stat -f %z "$tmp/$asset")"
    [ "$got_bytes" = "$bytes" ] ||
      { echo "Cua Driver $version: $asset is $got_bytes bytes, cua-driver.release.json says $bytes" >&2; exit 1; }
    got_sha="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
    [ "$got_sha" = "$sha256" ] ||
      { echo "Cua Driver $version: $asset has SHA-256 $got_sha, cua-driver.release.json says $sha256" >&2; exit 1; }
    mkdir -p "$tmp/x"
    tar -xzf "$tmp/$asset" -C "$tmp/x" "${files[@]}" ||
      { echo "Cua Driver $version: $asset lacks one of: ${files[*]}" >&2; exit 1; }
    mkdir -p "$dir"
    for f in "${files[@]}"; do install -m 0755 "$tmp/x/$f" "$dir/$f"; done
  fi
fi

for f in "${files[@]}"; do
  bin="$dir/$f"
  [ -x "$bin" ] || { echo "$bin is not an executable" >&2; exit 1; }
  if [ -z "${CUA_DRIVER_DIR:-}" ]; then
    codesign --verify --strict -R="anchor apple generic and certificate leaf[subject.OU] = \"$team\"" "$bin" 2>/dev/null ||
      { echo "$bin is not signed by Cua's Developer ID (team $team)" >&2; exit 1; }
    flags="$(codesign -dv "$bin" 2>&1 | sed -n 's/^CodeDirectory.*flags=\([^ ]*\).*/\1/p')"
    case "$flags" in *runtime*) ;; *) echo "$bin is not signed with the hardened runtime ($flags)" >&2; exit 1 ;; esac
  fi
  for arch in $archs; do
    lipo "$bin" -verify_arch "$arch" 2>/dev/null || { echo "$bin has no $arch slice" >&2; exit 1; }
  done
done
[ -n "${CUA_DRIVER_DIR:-}" ] || touch "$dir/.verified"
echo "$dir"
