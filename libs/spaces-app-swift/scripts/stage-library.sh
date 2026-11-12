#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Stages the Cua Spaces app export (libcua_spaces_ffi, which carries the MIT
# cua SDK too) where libs/cua/swift links its library in development
# (`lib/libcua_sdk.dylib`), so the Spaces apps and this package load one
# library for both bindings. Build it first:
#   cargo build --locked --release -p cua-spaces-ffi   (in libs/cua)
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cua="$here/../cua"
target="${CARGO_TARGET_DIR:-$cua/target}"
# CUA_CARGO_PROFILE_DIR: the Cargo profile directory (default release; dev-fast
# for install-local.sh --development).
profile_dir="${CUA_CARGO_PROFILE_DIR:-release}"
src="$target/$profile_dir/libcua_spaces_ffi.dylib"
[ -f "$src" ] || { echo "missing $src: cargo build --locked --profile $profile_dir -p cua-spaces-ffi" >&2; exit 1; }
dst="$cua/swift/lib/libcua_sdk.dylib"
mkdir -p "$(dirname "$dst")"
cp "$src" "$dst"
install_name_tool -id "@rpath/libcua_sdk.dylib" "$dst"
echo "staged $dst (the Cua Spaces app export)"
