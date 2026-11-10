#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# scripts/fetch-cua-driver.sh refuses a Cua Driver download whose size or
# SHA-256 differs from the committed table, one that lacks a listed file,
# and one that is not signed by the table's team. Uses a local archive and a
# temporary table: no network, no certificate.
#
#   scripts/tests/fetch-cua-driver-test.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"; rm -rf "$here/.build/cua-driver/9.9.9-test"' EXIT

mkdir -p "$work/src" "$work/srv"
printf '#!/bin/sh\necho driver\n' >"$work/src/cua-driver"
printf '#!/bin/sh\necho theme\n' >"$work/src/cua-cursor-theme"
chmod 0755 "$work/src/"*
asset=cua-driver-rs-9.9.9-test-darwin-universal-binary.tar.gz
tar -czf "$work/srv/$asset" -C "$work/src" cua-driver cua-cursor-theme
bytes="$(stat -f %z "$work/srv/$asset")"
sha="$(shasum -a 256 "$work/srv/$asset" | awk '{print $1}')"

table() { # bytes sha files-json
  cat >"$work/table.json" <<EOF
{"version": "9.9.9-test", "tag": "t", "asset": "$asset", "bytes": $1,
 "sha256": "$2", "teamId": "YCK386LBJ7", "files": $3}
EOF
}
fails_with() { # expected-message
  rm -rf "$here/.build/cua-driver/9.9.9-test"
  local out
  if out="$(CUA_DRIVER_RELEASE_TABLE="$work/table.json" CUA_DRIVER_RELEASE_BASE_URL="file://$work/srv" \
    "$here/scripts/fetch-cua-driver.sh" arm64 2>&1)"; then
    echo "FAIL: accepted, wanted: $1" >&2
    exit 1
  fi
  case "$out" in *"$1"*) echo "ok: $1" ;; *) echo "FAIL: wanted '$1', got: $out" >&2; exit 1 ;; esac
}

table $((bytes + 1)) "$sha" '["cua-driver", "cua-cursor-theme"]'
fails_with "bytes, cua-driver.release.json says $((bytes + 1))"

table "$bytes" "0000000000000000000000000000000000000000000000000000000000000000" '["cua-driver", "cua-cursor-theme"]'
fails_with "has SHA-256 $sha"

table "$bytes" "$sha" '["cua-driver", "cua-cursor-theme", "missing-helper"]'
fails_with "lacks one of"

# Size and checksum match, but the files are unsigned shell scripts.
table "$bytes" "$sha" '["cua-driver", "cua-cursor-theme"]'
fails_with "is not signed by Cua's Developer ID (team YCK386LBJ7)"

# The committed table names a real release asset and every field.
for k in version tag asset bytes sha256 teamId files; do
  plutil -extract "$k" raw -o - "$here/cua-driver.release.json" >/dev/null
done
v="$(plutil -extract version raw -o - "$here/cua-driver.release.json")"
[ "$(plutil -extract tag raw -o - "$here/cua-driver.release.json")" = "cua-driver-rs-v$v" ]
[ "$(plutil -extract asset raw -o - "$here/cua-driver.release.json")" = "cua-driver-rs-$v-darwin-universal-binary.tar.gz" ]
echo "ok: the committed table is consistent ($v)"
