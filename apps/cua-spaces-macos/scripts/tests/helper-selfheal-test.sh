#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# CUA-3746: the host service's login item under real launchd, through a
# Sparkle-style bundle swap. A fake app (the built CuaSpacesMac as its
# launcher, a shell script as Contents/Helpers/cua-spacesd) is loaded as a
# LaunchAgent with its own label and home under a scratch folder; nothing of
# an installed Cua Spaces (its label, ~/.cua, its keychain items) is touched.
#
#   1. a swap (new files at the same paths) restarts it onto the new daemon;
#   2. the app's files gone for longer than the watch's two looks, with the
#      host still set up, is a retry, not a stop: it runs again once the
#      files are back (before the fix it exited 0 and stayed stopped);
#   3. a SIGTERM in the first seconds of a start (a restarter fighting the
#      update) comes back; a stop later on stays stopped; the host removed
#      (spec gone) stays stopped.
#
# Usage: scripts/tests/helper-selfheal-test.sh /path/to/CuaSpacesMac
# Needs a logged-in macOS user session (launchd's gui domain). ~1 minute.
set -euo pipefail

exe="${1:?usage: $0 /path/to/CuaSpacesMac}"
[[ -x "$exe" ]] || { echo "not executable: $exe" >&2; exit 2; }
[[ "$(uname)" == Darwin ]] || { echo "macOS only" >&2; exit 2; }

work="$(mktemp -d "${TMPDIR:-/tmp}/cua3746-sim.XXXXXX")"
label="com.trycua.test.cua3746.$$"
domain="gui/$(id -u)"
app="$work/Fake.app"
home="$work/home"
spec="$home/helper/service.json"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers" "$home/helper"

cleanup() {
  launchctl bootout "$domain/$label" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

cp "$exe" "$app/Contents/MacOS/CuaSpacesMac"
cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.trycua.test.cua3746</string>
<key>CFBundleExecutable</key><string>CuaSpacesMac</string>
<key>CuaHelperLabel</key><string>$label</string>
<key>CuaHome</key><string>$home</string>
<key>CuaChannel</key><string>test</string>
</dict></plist>
PLIST

# The fake cua-spacesd: records its generation and runs until killed.
daemon() { # generation
  cat >"$app/Contents/Helpers/cua-spacesd.new" <<SH
#!/bin/sh
echo "$1 \$\$" > "$work/running"
trap 'exit 0' TERM
while :; do sleep 1; done
SH
  chmod +x "$app/Contents/Helpers/cua-spacesd.new"
  mv "$app/Contents/Helpers/cua-spacesd.new" "$app/Contents/Helpers/cua-spacesd"
}
daemon gen1
cat >"$spec" <<JSON
{"args": ["--no-desktop"], "env": [], "log_path": "$home/logs/host.log", "working_dir": "$work"}
JSON

plist="$work/$label.plist"
cat >"$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>$label</string>
<key>ProgramArguments</key><array>
<string>$app/Contents/MacOS/CuaSpacesMac</string><string>--service-helper</string><string>run</string></array>
<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
<key>ThrottleInterval</key><integer>1</integer>
<key>StandardErrorPath</key><string>$work/launcher.err</string>
</dict></plist>
PLIST

fail() { echo "FAIL: $*" >&2; echo "--- launcher.err" >&2; cat "$work/launcher.err" >&2 || true; exit 1; }
gen() { cut -d' ' -f1 "$work/running" 2>/dev/null || true; }
pid() { launchctl print "$domain/$label" 2>/dev/null | sed -n 's/^[[:space:]]*pid = //p' | head -1; }
state() { launchctl print "$domain/$label" 2>/dev/null | sed -n 's/^[[:space:]]*state = //p' | head -1; }
wait_for() { # description timeout command...
  local what="$1" t="$2"; shift 2
  for _ in $(seq 1 "$t"); do "$@" && return 0; sleep 1; done
  fail "$what"
}
is_gen() { [[ "$(gen)" == "$1" ]]; }
is_running() { [[ "$(state)" == running ]]; }
is_stopped() { [[ "$(state)" != running ]]; }

launchctl bootstrap "$domain" "$plist"
wait_for "the service runs gen1" 20 is_gen gen1
echo "ok: runs gen1 (pid $(pid))"

# 1. A swap: new files at the same paths.
cp "$app/Contents/MacOS/CuaSpacesMac" "$work/launcher.new"
mv "$work/launcher.new" "$app/Contents/MacOS/CuaSpacesMac"
daemon gen2
wait_for "the swap restarts it onto gen2" 40 is_gen gen2
wait_for "and it runs" 10 is_running
echo "ok: a swap restarts it onto the new daemon"

# 2. The files gone for longer than the watch's two looks (>= 10 s), the
# host still set up. Before the fix the launcher exited 0 and launchd left
# it stopped for good.
mv "$app/Contents/Helpers/cua-spacesd" "$work/cua-spacesd.away"
sleep 14
mv "$work/cua-spacesd.away" "$app/Contents/Helpers/cua-spacesd"
wait_for "it runs again once the files are back" 60 is_running
wait_for "and runs the daemon" 20 is_gen gen2
echo "ok: files gone while the host is set up is a retry"

# 3a. SIGTERM in the first seconds of a start: comes back.
launchctl kickstart -k "$domain/$label"
sleep 1
launchctl kill SIGTERM "$domain/$label" || true
sleep 4
wait_for "a SIGTERM right after a start comes back" 40 is_running
echo "ok: a stop in the first seconds of a start comes back"

# 3b. A stop long after the start stays stopped.
sleep 8
launchctl kill SIGTERM "$domain/$label"
sleep 5
is_stopped || fail "a stop after the start grace should stay stopped"
echo "ok: a stop later stays stopped"

# 3c. The host removed (spec gone) stays stopped.
launchctl kickstart "$domain/$label"
wait_for "it runs again" 20 is_running
rm -f "$spec"
sleep 16
is_stopped || fail "the spec gone should stop it for good"
echo "ok: the host removed stays stopped"

echo "all passed"
