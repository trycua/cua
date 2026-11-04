#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Prints the Keyvault trusted-caller requirement for a development build of
# this app, for a debug `cua daemon`:
#
#   CUA_KEYVAULT_TEST_REQUIREMENT="$(scripts/dev-keyvault.sh "<path>/Cua Spaces.app")" \
#     cua daemon start --foreground
#
# The broker then treats exactly this build (by cdhash) as first party.
# Release daemons ignore the variable and require Cua's production
# requirement (team YCK386LBJ7 and an identifier in CUA_IDENTIFIERS); see
# README "Keyvault identity". Extra binaries (for example a debug `cua` for
# `cua keyvault`, or a fixture tool) can follow the app path and are OR-ed
# in. A daemon with a test identity is passphrase-only: it never touches the
# login keychain.
set -euo pipefail
[ $# -ge 1 ] || { echo "usage: $0 <Cua Spaces.app> [binary ...]" >&2; exit 2; }
req=""
for path in "$@"; do
  hash="$(codesign -dvvv "$path" 2>&1 | sed -n 's/^CDHash=//p' | head -1)"
  [ -n "$hash" ] || { echo "$path is not signed" >&2; exit 1; }
  req="${req:+$req or }cdhash H\"$hash\""
done
echo "$req"
