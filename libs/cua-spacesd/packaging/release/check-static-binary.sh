#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Release gate for the static (musl) headless cua-spacesd that Cua injects into
# OCI images: fully static (no interpreter, no NEEDED libraries), starts, and
# runs a command under its PID 1 supervisor with that command's exit status.
#
#   check-static-binary.sh PATH
set -euo pipefail
bin="$1"
[ -f "$bin" ] || { echo "$bin does not exist" >&2; exit 2; }

# A static-pie executable reads as "dynamically linked" to older `file`
# (debian:11), so ask the program headers: no interpreter means static.
if readelf -l "$bin" 2>/dev/null | grep -q "INTERP"; then
  echo "$bin is dynamically linked; the injected daemon must be static" >&2
  file "$bin" >&2
  exit 1
fi
if readelf -d "$bin" 2>/dev/null | grep -q NEEDED; then
  echo "$bin has NEEDED libraries" >&2
  readelf -d "$bin" | grep NEEDED >&2
  exit 1
fi
file "$bin"
"$bin" --version

# The supervisor: the command's status is the Space's, the token is not in
# the command's environment, and cua-spacesd serves beside it.
set +e
CUA_ENV_TOKEN=release-check CUA_ENV_LISTEN=127.0.0.1:0 \
  "$bin" supervise -- /bin/sh -c 'test -z "$CUA_ENV_TOKEN" && exit 7'
code=$?
set -e
if [ "$code" != 7 ]; then
  echo "supervise exited $code, expected the command's 7" >&2
  exit 1
fi
echo "static build OK"
