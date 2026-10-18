#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Submits .zip, .dmg or .pkg files to Apple's notary service and waits. Given
# several files it notarizes them in parallel. On any result but Accepted it
# prints the notary log (which names each rejected file and why) and fails.
# Stapling is the caller's next step.
#
# Apple-side trouble does not fail the run: a submit that errors (5xx, timeout,
# timestamp server) is submitted again, and a wait that errors is resumed on
# the same submission, each with backoff (scripts/retry-apple.sh). A result of
# Invalid or Rejected is final.
#
#   APPLE_ID=... APPLE_PASSWORD=<app-specific password> APPLE_TEAM_ID=... \
#     scripts/notarize.sh <file> [<file>...]
set -euo pipefail
[ $# -ge 1 ] || { echo "usage: notarize.sh <file.zip|.dmg|.pkg>..." >&2; exit 2; }
: "${APPLE_ID:?}" "${APPLE_PASSWORD:?}" "${APPLE_TEAM_ID:?}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ $# -gt 1 ]; then
  pids=()
  for f in "$@"; do
    "$0" "$f" &
    pids+=($!)
  done
  failed=0
  for pid in "${pids[@]}"; do wait "$pid" || failed=1; done
  exit "$failed"
fi

file="$1"
auth=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
result="$(mktemp)"
trap 'rm -f "$result"' EXIT

# Submit (no --wait), so a failure while waiting does not lose the submission.
"$here/retry-apple.sh" -- xcrun notarytool submit "$file" "${auth[@]}" --output-format json >"$result" || true
id="$(plutil -extract id raw -o - "$result" 2>/dev/null || true)"
if [ -z "$id" ]; then
  echo "notarization of ${file##*/}: not submitted" >&2
  cat "$result" >&2
  exit 1
fi
echo "notarization of ${file##*/}: submitted ($id)"

status=""
for attempt in 1 2 3 4 5 6; do
  xcrun notarytool wait "$id" "${auth[@]}" --timeout 90m --output-format json >"$result" 2>/dev/null || true
  status="$(plutil -extract status raw -o - "$result" 2>/dev/null || true)"
  case "$status" in Accepted | Invalid | Rejected) break ;; esac
  # No verdict yet (the wait timed out or the service answered an error):
  # ask again about the same submission.
  info="$(xcrun notarytool info "$id" "${auth[@]}" --output-format json 2>/dev/null || true)"
  status="$(printf '%s' "$info" | plutil -extract status raw -o - - 2>/dev/null || true)"
  case "$status" in Accepted | Invalid | Rejected) break ;; esac
  echo "::warning::notarization of ${file##*/} ($id) has no verdict yet (attempt $attempt of 6); asking again in $((attempt * 30))s" >&2
  sleep $((attempt * 30))
done
echo "notarization of ${file##*/}: ${status:-no result} (submission $id)"
if [ "$status" != Accepted ]; then
  cat "$result" >&2
  "$here/retry-apple.sh" -- xcrun notarytool log "$id" "${auth[@]}" >&2 || true
  exit 1
fi
