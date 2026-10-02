#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Submits a .zip, .dmg or .pkg to Apple's notary service and waits. On any
# result but Accepted it prints the notary log (which names each rejected
# file and why) and fails. Stapling is the caller's next step.
#
#   APPLE_ID=... APPLE_PASSWORD=<app-specific password> APPLE_TEAM_ID=... \
#     scripts/notarize.sh <file>
set -euo pipefail
file="${1:?usage: notarize.sh <file.zip|.dmg|.pkg>}"
: "${APPLE_ID:?}" "${APPLE_PASSWORD:?}" "${APPLE_TEAM_ID:?}"
auth=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
result="$(mktemp)"
trap 'rm -f "$result"' EXIT
xcrun notarytool submit "$file" "${auth[@]}" --wait --timeout 90m --output-format json >"$result" || true
id="$(plutil -extract id raw -o - "$result" 2>/dev/null || true)"
status="$(plutil -extract status raw -o - "$result" 2>/dev/null || true)"
echo "notarization of ${file##*/}: ${status:-no result} (submission ${id:-none})"
if [ "$status" != Accepted ]; then
  cat "$result" >&2
  [ -n "$id" ] && xcrun notarytool log "$id" "${auth[@]}" >&2 || true
  exit 1
fi
