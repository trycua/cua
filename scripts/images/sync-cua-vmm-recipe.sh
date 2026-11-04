#!/usr/bin/env bash
# Refresh the copies of the macOS recipe files that cua-vmm embeds
# (libs/cua/crates/cua-vmm/recipe/), from libs/images/macos. A test in
# scripts/images/tests fails when they differ.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
src="$root/libs/images/macos"
dst="$root/libs/cua/crates/cua-vmm/recipe"
mkdir -p "$dst"
cp "$src/derive-local.sh" "$dst/macos-derive-local.sh"
cp "$src/versions.env" "$dst/macos-versions.env"
cp "$src/files/tier-xcode.sh" "$dst/macos-tier-xcode.sh"
cp "$src/files/fetch.sh" "$dst/macos-fetch.sh"
cp "$src/files/xcode-source.sh" "$dst/macos-xcode-source.sh"
