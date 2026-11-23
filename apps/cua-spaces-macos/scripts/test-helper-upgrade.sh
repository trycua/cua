#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Upgrade test for the host service's move into the app ("Cua Spaces
# Helper"). Run it on a disposable Mac (a VM), never on a Mac you use: it
# replaces /Applications/Cua Spaces.app and moves this Mac's host service.
#
# Before: an older Cua Spaces (0.9.4 or earlier) in /Applications, signed
# in, and this Mac set up for access with its `cua host setup` (the
# LaunchAgent com.trycua.spacesd.host running ~/.cua/host/bin/cua-spacesd),
# online at the relay.
#
#   scripts/test-helper-upgrade.sh upgrade NEW.app      install NEW.app over the old app, `cua host migrate`
#   scripts/test-helper-upgrade.sh update NEWER.app     replace the app again (an update): the helper and
#                                                       its permissions stay
#   scripts/test-helper-upgrade.sh downgrade OLD.app    put the old app back: nothing left broken, and
#                                                       `cua host setup` brings the old service back with
#                                                       the same machine id
#   scripts/test-helper-upgrade.sh remember             record the machine before anything (the
#                                                       first upgrade does it too)
#   scripts/test-helper-upgrade.sh check                the checks alone (after the app moved it)
#
# Each check prints PASS or FAIL; the exit status is the number of FAILs.
# The state before the first step is kept in ~/.cua-helper-test.
set -uo pipefail
app="/Applications/Cua Spaces.app"
cua="$app/Contents/MacOS/cua"
state="$HOME/.cua-helper-test"
label_old=com.trycua.spacesd.host
label_new=com.trycua.spaces.helper
gui="gui/$(id -u)"
fails=0
export CUA_TELEMETRY=0

pass() { printf 'PASS  %s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*"; fails=$((fails + 1)); }
check() { if eval "$2"; then pass "$1"; else fail "$1"; fi; }

status_json() { "$cua" host status --json 2>/dev/null; }
field() { python3 -c 'import json,sys; d=json.load(sys.stdin); v=d
for k in sys.argv[1].split("."): v=v.get(k) if isinstance(v,dict) else None
print("" if v is None else (str(v).lower() if isinstance(v,bool) else v))' "$1"; }

remember() {
  mkdir -p "$state"
  [ -f "$state/machine-id" ] && return 0
  status_json | field machineId > "$state/machine-id"
  shasum -a 256 "$HOME/.cua/host/machine-token" | awk '{print $1}' > "$state/token"
  "$cua" spaces ls 2>/dev/null | awk '{print $1}' | sort > "$state/spaces"
  echo "remembered machine $(cat "$state/machine-id")"
}

replace_app() {
  local src="$1"
  osascript -e 'quit app "Cua Spaces"' >/dev/null 2>&1 || true
  pkill -x CuaSpacesMac 2>/dev/null || true
  sleep 2
  rm -rf "$app"
  ditto "$src" "$app"
}

wait_online() {
  for _ in $(seq 1 30); do
    [ "$(status_json | field online)" = true ] && return 0
    sleep 2
  done
  return 1
}

same_machine_checks() {
  local s
  s="$(status_json)"
  check "same machine id ($(cat "$state/machine-id"))" \
    '[ "$(printf %s "$s" | field machineId)" = "$(cat "$state/machine-id")" ]'
  check "same machine token (not registered again)" \
    '[ "$(shasum -a 256 "$HOME/.cua/host/machine-token" | awk "{print \$1}")" = "$(cat "$state/token")" ]'
  check "relay reachable: online" 'wait_online'
  check "Spaces still listed" \
    '[ -z "$(comm -23 "$state/spaces" <("$cua" spaces ls 2>/dev/null | awk "{print \$1}" | sort))" ]'
  check "this Mac answers through the relay" \
    '"$cua" sandbox exec "relay:$(cat "$state/machine-id")" -- echo ok 2>/dev/null | grep -qx ok'
}

# Whether LaunchServices lists a cua-spacesd under the app's bundle id: it
# then owns the app's launch (opening Cua Spaces activates it, no window).
spacesd_is_the_app() {
  lsappinfo list | awk '
    /bundleID=/ { id = $0 }
    /executable path=/ { if (id ~ /"com\.trycua\.spaces\.macos"/ && $0 ~ /\/cua-spacesd"/) found = 1 }
    END { exit !found }'
}

helper_checks() {
  local s
  s="$(status_json)"
  check "service kind is app-helper" '[ "$(printf %s "$s" | field service.kind)" = app-helper ]'
  check "the app's login item is running" 'launchctl print "$gui/$label_new" 2>/dev/null | grep -q "state = running"'
  check "the app's own executable runs it" 'pgrep -f "CuaSpacesMac --service-helper run" >/dev/null'
  check "cua-spacesd runs inside the app" \
    'pgrep -fl "Cua Spaces.app/Contents/Helpers/cua-spacesd" >/dev/null'
  check "cua-spacesd is not registered as the app (opening it shows a window)" '! spacesd_is_the_app'
  check "the old agent is gone from launchd" '! launchctl print "$gui/$label_old" >/dev/null 2>&1'
  check "the old plist is gone" '[ ! -e "$HOME/Library/LaunchAgents/$label_old.plist" ]'
  check "the old binary is gone" '[ ! -e "$HOME/.cua/host/bin/cua-spacesd" ]'
  check "no second cua-spacesd" '[ "$(pgrep -x cua-spacesd | wc -l | tr -d " ")" = 1 ]'
  check "the app is signed as com.trycua.spaces.macos with a stable requirement" \
    'codesign -d -r- "$app" 2>&1 | grep -q "identifier \"com.trycua.spaces.macos\"" &&
     ! codesign -d -r- "$app" 2>&1 | grep -q cdhash'
  check "no separate helper bundle" '[ ! -e "$app/Contents/Library/LoginItems" ]'
}

permissions() { status_json | python3 -c 'import json,sys; print(",".join(p["id"] for p in json.load(sys.stdin).get("permissions",[])))'; }

case "${1:-}" in
upgrade)
  new="${2:?usage: upgrade NEW.app}"
  remember
  replace_app "$new"
  open -a "$app"
  sleep 15
  "$cua" host migrate; echo "migrate exit $?"
  same_machine_checks
  helper_checks
  echo "permissions still to allow for Cua Spaces: $(permissions)"
  ;;
update)
  newer="${2:?usage: update NEWER.app}"
  before="$(permissions)"
  replace_app "$newer"
  open -a "$app"
  sleep 15
  launchctl kickstart -k "$gui/$label_new" >/dev/null 2>&1
  sleep 5
  same_machine_checks
  helper_checks
  check "permissions unchanged by the update (before: '${before}')" '[ "$(permissions)" = "$before" ]'
  ;;
downgrade)
  old="${2:?usage: downgrade OLD.app}"
  replace_app "$old"
  # The service notices its cua-spacesd is gone within about 10 s and stops.
  check "no service left running" \
    'for _ in $(seq 1 15); do pgrep -f "service-helper run" >/dev/null || break; sleep 2; done; ! pgrep -f "service-helper run" >/dev/null'
  check "the old app still reads this machine's setup" '[ "$(status_json | field configured)" = true ]'
  "$cua" host setup --name "$(status_json | field name)" >/dev/null 2>&1; echo "setup exit $?"
  open -a "$app"
  sleep 15
  check "the old agent runs again" 'launchctl print "$gui/$label_old" 2>/dev/null | grep -q "state = running"'
  check "same machine id after setup" '[ "$(status_json | field machineId)" = "$(cat "$state/machine-id")" ]'
  check "relay reachable: online" 'wait_online'
  check "this Mac answers through the relay" \
    '"$cua" sandbox exec "relay:$(cat "$state/machine-id")" -- echo ok 2>/dev/null | grep -qx ok'
  ;;
remember)
  remember
  ;;
check)
  same_machine_checks
  helper_checks
  ;;
*)
  sed -n '5,25p' "$0"
  exit 2
  ;;
esac
echo "$fails failed"
exit "$fails"
