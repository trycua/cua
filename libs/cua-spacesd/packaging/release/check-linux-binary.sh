#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Release gate for a Linux cua-spacesd binary: its newest GLIBC_ symbol
# must not exceed the floor (default 2.31, debian:11), OpenSSL must be
# linked statically (no libssl/libcrypto in the dynamic section), and it
# must start (`--version`).
#
#   check-linux-binary.sh PATH [MAX_GLIBC]
set -euo pipefail
bin="$1" floor="${2:-2.31}"
[ -f "$bin" ] || { echo "$bin does not exist" >&2; exit 2; }

newest="$(objdump -T "$bin" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)"
echo "newest GLIBC symbol: ${newest:-none} (floor $floor)"
if [ -n "$newest" ] && [ "$(printf '%s\n%s\n' "$newest" "$floor" | sort -V | tail -1)" != "$floor" ]; then
  echo "$bin needs GLIBC_$newest, above the $floor floor" >&2
  exit 1
fi

needed="$(objdump -p "$bin" | awk '/NEEDED/ {print $2}')"
echo "NEEDED:"; while read -r lib; do echo "  $lib"; done <<<"$needed"
if echo "$needed" | grep -Eq '^lib(ssl|crypto)\.so'; then
  echo "$bin links OpenSSL dynamically; the vendored (static) build is required" >&2
  exit 1
fi

"$bin" --version
