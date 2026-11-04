#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# The phone's wire, end to end, against the local harness (loopback relay,
# fake identity provider, one machine with its own daemon; see
# mobile-harness.sh). A script plays the phone (phone-probe.py: an enrolled
# device, gRPC-Web through the relay). Run `mobile-harness.sh start` first.
#
# What runs: device enrollment and session, the machine's tool offer, a push
# registration, an approval that becomes an alert (the relay's log provider
# writes it), a replay-safe answer, and the changes of a git repository in a
# Space (a real cua-spacesd on loopback standing in for a Space: no container
# or VM is started). Everything lives under $HARNESS_DIR; the processes it
# starts are stopped on exit.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
D="${HARNESS_DIR:-$HOME/projects/.cua-work/mobile-gaps/harness}"
[ -f "$D/env.sh" ] || { echo "start the harness first: mobile-harness.sh start" >&2; exit 1; }
# shellcheck disable=SC1091
. "$D/env.sh"
P="$here/phone-probe.py"
bin="$D/bin"; fake_home="$D/home"; cua_home="$D/cua-home"
failfile="$(mktemp)"
fail() { echo "FAIL  $1"; echo "$1" >>"$failfile"; }
check() { # name, command that must succeed
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then echo "ok    $name"; else fail "$name"; fi
}
jq_() { python3 -c 'import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))' "$1"; }
expect() { # name, expression over d (stdin), expected
  local got; got="$(cat | jq_ "$2" 2>&1 || true)"
  if [ "$got" = "$3" ]; then echo "ok    $1"; else fail "$1: got [$got], wanted [$3]"; fi
}
machine_env() {
  echo "HOME=$fake_home CUA_HOME=$cua_home CUA_CREDENTIAL_STORE=file CUA_RELAY_URL=$CUA_HARNESS_RELAY_URL CUA_OIDC_ISSUER=$CUA_HARNESS_ISSUER CUA_MESH=off CUA_TELEMETRY=off PATH=$bin:/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin"
}
cua() { env -i $(machine_env) "$bin/cua" "$@"; }

spacesd_pid=""
cleanup() {
  if [ -n "$spacesd_pid" ]; then
    kill "$spacesd_pid" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$spacesd_pid" 2>/dev/null || break; sleep 0.3; done
    kill -9 "$spacesd_pid" 2>/dev/null || true
  fi
  [ -n "${space:-}" ] && cua spaces remove "$space" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "== the phone: enroll, session, machines"
"$P" enroll >/dev/null
"$P" machines | expect "the machine is online" 'd[0]["online"]' True
"$P" call task_options '{}' | expect "task_options through the relay" 'd["result"]["machine"]["can_create"]' True

echo "== push: register, then an approval becomes an alert"
rm -f "$CUA_HARNESS_PUSH_ALERTS"
"$P" push-register | grep -q '^204' && echo "ok    registered" || fail "push-register"
approval="$("$P" call thread_post '{"text":"may I?","approval":{"action":"Install a package","details":"SECRET DETAIL"}}')"
seq="$(echo "$approval" | jq_ 'd["result"]["entry"]["seq"]')"
for _ in $(seq 1 40); do [ -s "$CUA_HARNESS_PUSH_ALERTS" ] && break; sleep 0.25; done
check "an alert was written" test -s "$CUA_HARNESS_PUSH_ALERTS"
tail -1 "$CUA_HARNESS_PUSH_ALERTS" | expect "the alert's kind and entry" 'd["payload"]["kind"], d["payload"]["entry"] == '"$seq" "('approval', True)"
tail -1 "$CUA_HARNESS_PUSH_ALERTS" | expect "no words of the request in it" '"SECRET" in json.dumps(d) or "pip" in json.dumps(d)' False
tail -1 "$CUA_HARNESS_PUSH_ALERTS" | expect "the alert is for the Allow and Deny category" 'd["payload"]["aps"]["category"]' CUA_APPROVAL

echo "== answers: once, replayed, then a second opinion"
a1="$("$P" call thread_post '{"text":"","answer":{"approval":'"$seq"',"approve":true,"client_id":"e2e-answer-0001"}}')"
echo "$a1" | expect "answered" 'd["result"]["entry"]["answer"]["approval"]' "$seq"
a2="$("$P" call thread_post '{"text":"","answer":{"approval":'"$seq"',"approve":true,"client_id":"e2e-answer-0001"}}')"
echo "$a2" | expect "the retry is the first answer" 'd["result"]["entry"]["replayed"], d["result"]["entry"]["seq"] == '"$(echo "$a1" | jq_ 'd["result"]["entry"]["seq"]')" "(True, True)"
"$P" call thread_post '{"text":"","answer":{"approval":'"$seq"',"approve":false}}' | expect "a different decision is refused" '"already answered (allowed)" in json.dumps(d)' True
"$P" call thread_read '{}' | expect "that approval is no longer open" "$seq in d[\"result\"][\"open_approvals\"]" False

echo "== changes of a repository in a Space (a real cua-spacesd, no container)"
repo="$fake_home/app"
rm -rf "$repo"; mkdir -p "$repo"
git -C "$repo" init -q -b main
printf 'one\ntwo\n' >"$repo/a.txt"
git -C "$repo" add . && git -C "$repo" -c user.name=t -c user.email=t@t commit -q -m first
printf 'one\nTWO\nthree\n' >"$repo/a.txt"; printf 'hello\n' >"$repo/new.md"
token="e2e-$(od -An -N8 -tx1 /dev/urandom | tr -d ' \n')"
port=$((RANDOM % 2000 + 40000))
( cd "$fake_home" && exec env -i HOME="$fake_home" CUA_ENV_TOKEN="$token" PATH="/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin" \
    "$bin/cua-spacesd" --listen "127.0.0.1:$port" --no-driver >"$D/logs/e2e-spacesd.log" 2>&1 ) &
spacesd_pid=$!
for _ in $(seq 1 50); do curl -fs -o /dev/null "http://127.0.0.1:$port/health" 2>/dev/null && break; sleep 0.2; done
space="direct:127.0.0.1:$port"
cua spaces add "http://127.0.0.1:$port" --token "$token" --name e2e-room >/dev/null 2>&1 || true
ch="$("$P" call space_changes '{"repo":"app"}' --space "$space")"
echo "$ch" | expect "the files, with counts" 'sorted((f["path"], f["status"], f["additions"], f["deletions"]) for f in d["result"]["files"])' "[('a.txt', 'modified', 2, 1), ('new.md', 'added', 1, 0)]"
echo "$ch" | expect "unverified, with the branch" 'd["result"]["verified"], d["result"]["branch"]' "(False, 'main')"
"$P" call space_file_diff '{"repo":"app","path":"a.txt"}' --space "$space" | expect "one file's diff" '"+TWO" in d["result"]["patch"] and "-two" in d["result"]["patch"]' True
"$P" call space_file_preview '{"repo":"app","path":"new.md"}' --space "$space" | expect "a preview" 'd["result"]["content"] == "hello\n"' True
"$P" call space_changes '{"repo":"nowhere"}' --space "$space" | expect "a missing folder says so" '"no such folder" in json.dumps(d)' True

echo
fails="$(wc -l <"$failfile" | tr -d ' ')"; rm -f "$failfile"
if [ "$fails" = 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi
