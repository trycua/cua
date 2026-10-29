#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Live check of the Cua Driver that Cua Spaces runs for coding agents
# (cua-spacesd's embedded child). Run it INSIDE a disposable macOS VM, never
# on a person's Mac: it turns the embedded driver on and drives the agent
# cursor. Before running it, in the VM: install the Cua Spaces build under
# test in /Applications, finish Set up, and allow Accessibility and Screen
# Recording for Cua Spaces when macOS asks (inside the VM only).
#
#   scripts/tests/embedded-driver-vm-check.sh [/Applications/Cua Spaces.app]
#
# Checks (each prints ok/FAIL; exit 1 on any FAIL):
#   1. Contents/Helpers has cua-driver and cua-cursor-theme, signed by the
#      Cua team; Contents/MacOS has no cua-driver.
#   2. With CUA_SPACESD_EMBEDDED_DRIVER=1, cua-spacesd starts
#      `cua-driver serve --embedded` and is its parent process.
#   3. The coding-agent route (`cua-driver mcp --embedded --socket ...`)
#      answers initialize, check_permissions reports both permissions on,
#      and move_cursor moves the agent cursor without moving the real
#      pointer.
#   4. Killing the driver: cua-spacesd starts a new one within 10 s.
#   5. Only Cua Spaces (no CuaDriver) is in the permission lists, when the
#      TCC databases are readable (needs sudo with Full Disk Access).
#
# Done by the person or agent running it, from the host side (VNC):
#   - during step 3, a screenshot shows the agent cursor near (200,200) and
#     then (900,500), and the real pointer did not move;
#   - only one agent cursor is drawn per agent (cua-spacesd's own overlay
#     and the driver's overlay don't double up or flicker);
#   - opening Cua Spaces from Finder still opens its window (the driver in
#     Contents/Helpers didn't take over the app's launch).
# ok/fail never fail, so `test && ok || fail` is an if/else here.
# shellcheck disable=SC2015
set -uo pipefail
app="${1:-/Applications/Cua Spaces.app}"
helpers="$app/Contents/Helpers"
bundle_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")"
case "$bundle_id" in
  com.trycua.spaces.macos.local) home="$HOME/.cua-local" ;;
  *) home="$HOME/.cua" ;;
esac
socket="$home/run/cua-driver.sock"
spec="$home/helper/service.json"
fails=0
ok() { echo "ok: $*"; }
fail() { echo "FAIL: $*"; fails=$((fails + 1)); }

# 1. Bundle layout and signatures.
for f in cua-driver cua-cursor-theme; do
  if codesign --verify --strict -R='anchor apple generic and certificate leaf[subject.OU] = "YCK386LBJ7"' "$helpers/$f" 2>/dev/null; then
    ok "$f is in Contents/Helpers, signed by the Cua team"
  else
    fail "$helpers/$f is missing or not signed by the Cua team"
  fi
done
[ ! -e "$app/Contents/MacOS/cua-driver" ] && ok "no cua-driver in Contents/MacOS" || fail "cua-driver is in Contents/MacOS"

# 2. Turn the embedded driver on in the service spec and restart the service.
[ -f "$spec" ] || { fail "no $spec (finish Set up in Cua Spaces first)"; exit 1; }
cp "$spec" "$spec.before-embedded-check"
# env is a list of [name, value] pairs.
plutil -insert env.0 -json '["CUA_SPACESD_EMBEDDED_DRIVER","1"]' "$spec" 2>/dev/null ||
  plutil -insert env -json '[["CUA_SPACESD_EMBEDDED_DRIVER","1"]]' "$spec"
pkill -x cua-spacesd || true # the app's login item starts it again with the new spec
driver_pid=""
for _ in $(seq 1 30); do
  driver_pid="$(pgrep -f "^$helpers/cua-driver serve --embedded" | head -1)"
  [ -n "$driver_pid" ] && break
  sleep 1
done
if [ -z "$driver_pid" ]; then
  fail "no 'cua-driver serve --embedded' from Contents/Helpers within 30 s"
else
  parent="$(ps -o ppid= -p "$driver_pid" | tr -d ' ')"
  parent_cmd="$(ps -o command= -p "$parent")"
  case "$parent_cmd" in
    "$helpers/cua-spacesd"*) ok "cua-driver (pid $driver_pid) is cua-spacesd's child (pid $parent)" ;;
    *) fail "cua-driver's parent is not cua-spacesd: $parent_cmd" ;;
  esac
  ps -o command= -p "$driver_pid" | grep -q -- "--host-bundle-id $bundle_id" &&
    ok "it runs for $bundle_id" || fail "it doesn't name $bundle_id as its host"
fi
[ -S "$socket" ] && ok "socket $socket" || fail "no socket at $socket"

# 3. The coding-agent route.
out="$(mktemp)"
{
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"vm-check","version":"1"}}}'
  printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"check_permissions","arguments":{"prompt":false}}}'
  printf '%s\n' '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"move_cursor","arguments":{"x":200,"y":200,"session":"vm-check"}}}'
  sleep 2
  printf '%s\n' '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"move_cursor","arguments":{"x":900,"y":500,"session":"vm-check"}}}'
  sleep 3
} | "$helpers/cua-driver" mcp --embedded --socket "$socket" --host-bundle-id "$bundle_id" >"$out" 2>"$out.err"
grep -q '"id":1,"result"' "$out" && ok "the agent route answers initialize" || fail "no initialize answer ($(tail -2 "$out.err"))"
perms="$(grep '"id":2' "$out")"
for key in accessibility screen_recording; do
  echo "$perms" | grep -Eq "\\\\?\"$key\\\\?\": *true" && ok "$key is on (as Cua Spaces)" || fail "$key is not on: $perms"
done
grep '"id":4' "$out" | grep -q '"isError":true' && fail "move_cursor failed: $(grep '"id":4' "$out")" || ok "move_cursor answered (check the screenshots)"
rm -f "$out" "$out.err"

# 4. Restart after a crash.
if [ -n "$driver_pid" ]; then
  kill -9 "$driver_pid"
  new=""
  for _ in $(seq 1 10); do
    sleep 1
    new="$(pgrep -f "^$helpers/cua-driver serve --embedded" | head -1)"
    [ -n "$new" ] && [ "$new" != "$driver_pid" ] && break
  done
  [ -n "$new" ] && [ "$new" != "$driver_pid" ] && ok "restarted after a crash (pid $new)" || fail "not restarted within 10 s"
fi

# 5. Permission lists (best effort).
for db in "/Library/Application Support/com.apple.TCC/TCC.db" "$HOME/Library/Application Support/com.apple.TCC/TCC.db"; do
  rows="$(sudo -n sqlite3 "$db" "select service, client from access where service in ('kTCCServiceAccessibility','kTCCServiceScreenCapture') and auth_value = 2" 2>/dev/null)" || continue
  echo "$rows" | grep -qi "cuadriver\|com.trycua.driver" && fail "a CuaDriver entry is allowed in $db: $rows" || ok "no CuaDriver entry in $db"
done

mv "$spec.before-embedded-check" "$spec"
echo "$fails check(s) failed"
[ "$fails" = 0 ]
