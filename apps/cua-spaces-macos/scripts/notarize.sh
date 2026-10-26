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
# So is a crash of notarytool itself. A signal kill (the "Bus error: 10" of
# #5837, exit status 138) prints nothing retry-apple.sh could match, and used
# to fail the job at its last step, after about two hours of build. A submit
# that dies from a signal before it returned a submission id is submitted
# again (3 attempts, 10 s then 20 s apart). Once an id exists the file is never
# submitted again: a crash while waiting resumes on that id (wait, then info).
#
#   APPLE_ID=... APPLE_PASSWORD=<app-specific password> APPLE_TEAM_ID=... \
#     scripts/notarize.sh <file> [<file>...]
#
# NOTARIZE_SUBMIT_ATTEMPTS (3), NOTARIZE_SUBMIT_BACKOFF (10, seconds, times the
# attempt number) and NOTARIZE_WAIT_BACKOFF (30, the same for resuming a wait)
# exist for scripts/tests/notarize-test.sh.
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

submit_attempts="${NOTARIZE_SUBMIT_ATTEMPTS:-3}"
submit_backoff="${NOTARIZE_SUBMIT_BACKOFF:-10}"
wait_backoff="${NOTARIZE_WAIT_BACKOFF:-30}"

# True when a signal killed the process (bash reports 128 + the signal number),
# but not when someone stopped it on purpose: hang-up, interrupt, terminate.
died_from_signal() {
  [ "$1" -gt 128 ] && [ "$1" -ne 129 ] && [ "$1" -ne 130 ] && [ "$1" -ne 143 ]
}

# Submit (no --wait), so a failure while waiting does not lose the submission.
# A crash can come before the upload ends, and then Apple has nothing to wait
# for, or after it, and then a second submission of the same file is harmless:
# the notary service accepts it as another submission.
id=""
rc=0
for attempt in $(seq 1 "$submit_attempts"); do
  rc=0
  "$here/retry-apple.sh" -- xcrun notarytool submit "$file" "${auth[@]}" --output-format json >"$result" || rc=$?
  id="$(plutil -extract id raw -o - "$result" 2>/dev/null || true)"
  [ -z "$id" ] || break
  if ! died_from_signal "$rc" || [ "$attempt" -ge "$submit_attempts" ]; then
    break
  fi
  echo "::warning::notarytool submit for ${file##*/} was killed by signal $((rc - 128)) (exit $rc) before it returned a submission id (attempt $attempt of $submit_attempts); submitting again in $((attempt * submit_backoff))s" >&2
  sleep $((attempt * submit_backoff))
done
if [ -z "$id" ]; then
  echo "notarization of ${file##*/}: not submitted (notarytool exit $rc)" >&2
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
  echo "::warning::notarization of ${file##*/} ($id) has no verdict yet (attempt $attempt of 6); asking again in $((attempt * wait_backoff))s" >&2
  sleep $((attempt * wait_backoff))
done
echo "notarization of ${file##*/}: ${status:-no result} (submission $id)"
if [ "$status" != Accepted ]; then
  cat "$result" >&2
  "$here/retry-apple.sh" -- xcrun notarytool log "$id" "${auth[@]}" >&2 || true
  exit 1
fi
