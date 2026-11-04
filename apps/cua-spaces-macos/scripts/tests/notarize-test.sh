#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Tests scripts/notarize.sh against a fake `xcrun` (#5837). Nothing here talks
# to Apple. It checks that:
#
#   - a submit killed by a signal before it returned a submission id is
#     submitted again, and the run goes on to Accepted;
#   - a submit that keeps dying stops after 3 attempts with a clear message;
#   - a submit that fails by itself (exit 1) or was stopped on purpose (exit
#     130) is not repeated;
#   - once a submission id exists, a wait killed by a signal resumes on that id
#     (wait, then info) and the file is never submitted again;
#   - a verdict of Invalid is final: one submit, the notary log is fetched, the
#     run fails.
#
#   scripts/tests/notarize-test.sh
#
# Each check prints PASS or FAIL; the exit status is the number of FAILs.
# It needs bash, python3 and plutil (macOS) or python3 alone (a shim stands in
# for plutil on any other system, or when CUA_TEST_FAKE_PLUTIL=1).
# The checks are strings for eval: the variables they read are assigned above,
# and shellcheck cannot see that.
# shellcheck disable=SC2034,SC2016,SC2329
set -uo pipefail

TESTS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd -- "$TESTS_DIR/.." && pwd)"

fails=0
pass() { printf 'PASS  %s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*"; fails=$((fails + 1)); }
check() { if eval "$2"; then pass "$1"; else fail "$1"; fi; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-notarize-test.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
STATE="$WORK/state"
mkdir -p "$SHIMS" "$STATE"
echo "dmg" >"$WORK/Cua-Spaces.dmg"

# The fake xcrun. FAKE_SUBMIT_CRASHES submits die from SIGKILL (exit 137), then
# submits answer with an id. FAKE_SUBMIT_RC makes a submit exit with that status
# and no output. FAKE_WAIT_CRASHES waits die from SIGKILL; FAKE_VERDICT is the
# status a wait or info reports. Every call is logged as its notarytool verb.
cat >"$SHIMS/xcrun" <<'SHIM'
#!/usr/bin/env bash
[ "$1" = notarytool ] || exit 99
verb="$2"
echo "$verb" >>"$STATE/calls"
n=$(grep -c "^$verb\$" "$STATE/calls")
case "$verb" in
  submit)
    [ -z "${FAKE_SUBMIT_RC:-}" ] || exit "$FAKE_SUBMIT_RC"
    [ "$n" -gt "${FAKE_SUBMIT_CRASHES:-0}" ] || kill -KILL $$
    echo '{"id":"abc-123","message":"Successfully uploaded file"}'
    ;;
  wait)
    [ "$n" -gt "${FAKE_WAIT_CRASHES:-0}" ] || kill -KILL $$
    echo "{\"id\":\"abc-123\",\"status\":\"${FAKE_VERDICT:-Accepted}\"}"
    ;;
  info) echo "{\"id\":\"abc-123\",\"status\":\"${FAKE_VERDICT:-Accepted}\"}" ;;
  log) echo '{"issues":[{"path":"Cua-Spaces.dmg","message":"fake log"}]}' ;;
esac
SHIM
chmod +x "$SHIMS/xcrun"

# plutil with -extract is macOS only (a plutil on a Linux runner, from the Swift
# toolchain, lacks it), so anything but macOS gets this shim. It reads one
# top-level key of a JSON file or stdin: plutil -extract KEY raw -o - FILE|-
if [ "${CUA_TEST_FAKE_PLUTIL:-0}" = 1 ] || [ "$(uname -s)" != Darwin ]; then
  cat >"$SHIMS/plutil" <<'SHIM'
#!/usr/bin/env bash
[ "$1" = -extract ] || exit 99
exec python3 -I -c '
import json, sys
key, path = sys.argv[1], sys.argv[2]
data = json.load(sys.stdin if path == "-" else open(path))
if not isinstance(data, dict) or key not in data:
    sys.exit(1)
print(data[key])
' "$2" "$6"
SHIM
  chmod +x "$SHIMS/plutil"
fi

# run_notarize <env assignments...>: runs notarize.sh on the fake disk image.
# The call log is in $STATE/calls, the output in $WORK/out, the status in $RC.
RC=0
run_notarize() {
  : >"$STATE/calls"
  env STATE="$STATE" PATH="$SHIMS:$PATH" \
    APPLE_ID=test@example.com APPLE_PASSWORD=not-a-password APPLE_TEAM_ID=TEAM123456 \
    NOTARIZE_SUBMIT_BACKOFF=0 NOTARIZE_WAIT_BACKOFF=0 "$@" \
    "$SCRIPTS_DIR/notarize.sh" "$WORK/Cua-Spaces.dmg" >"$WORK/out" 2>&1
  RC=$?
}
calls() { grep -c "^$1\$" "$STATE/calls" || true; }

echo "# a crash before a submission id"
run_notarize FAKE_SUBMIT_CRASHES=1
check "one crash, then it submits again and the file is Accepted" \
  '[ "$RC" -eq 0 ] && [ "$(calls submit)" -eq 2 ] && grep -q "Accepted (submission abc-123)" "$WORK/out"'
check "the crash is logged as a warning with the signal" \
  'grep -q "::warning::notarytool submit for Cua-Spaces.dmg was killed by signal 9" "$WORK/out"'

run_notarize FAKE_SUBMIT_CRASHES=2
check "two crashes, then it submits a third time and is Accepted" \
  '[ "$RC" -eq 0 ] && [ "$(calls submit)" -eq 3 ]'

run_notarize FAKE_SUBMIT_CRASHES=99
check "it gives up after 3 attempts, fails, and says nothing was submitted" \
  '[ "$RC" -eq 1 ] && [ "$(calls submit)" -eq 3 ] && [ "$(calls wait)" -eq 0 ] && grep -q "not submitted (notarytool exit 137)" "$WORK/out"'

run_notarize FAKE_SUBMIT_CRASHES=99 NOTARIZE_SUBMIT_ATTEMPTS=1
check "NOTARIZE_SUBMIT_ATTEMPTS=1 never repeats a submit" \
  '[ "$RC" -eq 1 ] && [ "$(calls submit)" -eq 1 ]'

echo "# failures that are not crashes"
run_notarize FAKE_SUBMIT_RC=1
check "a submit that exits 1 is not repeated" \
  '[ "$RC" -eq 1 ] && [ "$(calls submit)" -eq 1 ] && grep -q "not submitted (notarytool exit 1)" "$WORK/out"'

run_notarize FAKE_SUBMIT_RC=130
check "a submit stopped on purpose (exit 130) is not repeated" \
  '[ "$RC" -eq 1 ] && [ "$(calls submit)" -eq 1 ]'

echo "# a crash after a submission id"
run_notarize FAKE_WAIT_CRASHES=1
check "a wait killed by a signal resumes on the same id and never submits again" \
  '[ "$RC" -eq 0 ] && [ "$(calls submit)" -eq 1 ] && [ "$(calls wait)" -eq 1 ] && [ "$(calls info)" -eq 1 ] && grep -q "Accepted (submission abc-123)" "$WORK/out"'

run_notarize FAKE_VERDICT=Invalid
check "an Invalid verdict is final: one submit, the log is fetched, the run fails" \
  '[ "$RC" -eq 1 ] && [ "$(calls submit)" -eq 1 ] && [ "$(calls log)" -eq 1 ] && grep -q "Invalid (submission abc-123)" "$WORK/out"'

echo
if [ "$fails" -eq 0 ]; then echo "all checks passed"; else echo "$fails check(s) failed"; fi
exit "$fails"
