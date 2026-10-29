#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# The host service (`CuaSpacesMac --service-helper run`) must never check in
# with LaunchServices as the app: while a windowless helper holds the app's
# entry, opening Cua Spaces fails (error -600). Runs the bundle's helper in a
# temporary home with crash reporting allowed (the Sentry SDK did the check-in
# in 0.17.0-beta.3 and beta.4) and a local-only cua-spacesd, then asks
# LaunchServices whether the helper's pid is an app.
#
#   scripts/tests/helper-not-an-app.sh "path/Cua Spaces.app"
set -euo pipefail
app="${1:?usage: helper-not-an-app.sh <Cua Spaces.app>}"
exe="$app/Contents/MacOS/CuaSpacesMac"
[ -x "$exe" ] || { echo "no $exe" >&2; exit 2; }

home="$(mktemp -d)"
pid=""
cleanup() {
  if [ -n "$pid" ]; then
    pkill -TERM -P "$pid" 2>/dev/null || true
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf "$home"
}
trap cleanup EXIT

mkdir -p "$home/.cua/helper" "$home/.cua/telemetry" "$home/data"
# Crash reporting allowed: the notice was shown and telemetry is on.
echo 1 >"$home/.cua/telemetry/notice_shown"
cat >"$home/.cua/helper/service.json" <<JSON
{"args": ["serve", "--listen", "127.0.0.1:0", "--no-desktop", "--no-driver", "--no-mcp", "--data-dir", "$home/data"],
 "env": [], "log": "$home/service.log"}
JSON

# No CI, DO_NOT_TRACK or CUA_TELEMETRY: they would turn crash reporting off
# and hide the check-in this test is for.
env -u CI -u GITHUB_ACTIONS -u DO_NOT_TRACK -u CUA_TELEMETRY HOME="$home" CUA_HOME="$home/.cua" \
  "$exe" --service-helper run >"$home/helper.out" 2>&1 &
pid=$!

registered=""
for _ in $(seq 1 16); do
  sleep 0.5
  kill -0 "$pid" 2>/dev/null || { echo "the helper exited early:" >&2; cat "$home/helper.out" "$home/service.log" >&2 2>/dev/null; exit 1; }
  registered="$(lsappinfo find "pid=$pid" 2>/dev/null || true)"
  [ -n "$registered" ] && break
done

if [ -n "$registered" ]; then
  echo "the host service (pid $pid) checked in with LaunchServices as an app:" >&2
  lsappinfo info "$registered" >&2 || true
  exit 1
fi
echo "ok: the host service ran for 8 s and never checked in as an app"
