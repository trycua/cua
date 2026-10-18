#!/bin/sh
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# The .deb's pre-removal script (electron-builder deb `fpm`: --before-remove).
#
# Cua Spaces puts things outside /opt that outlive the package: the host
# service ("Set up this machine for unattended access", a systemd user unit
# running a copy of cua-spacesd from ~/.cua) keeps the machine online on the
# Cua relay, the cua daemon keeps the Spaces running, and the app leaves its
# launch-at-login entry and a copy of `cua` in ~/.local/bin. Removing the
# package must turn all of that off, or the machine stays reachable through
# the relay with nobody able to see why (CUA-1521).
#
# Runs for `apt remove` and `apt purge` (both call prerm with "remove"), not
# for an upgrade. Per user that has a cua home (~/.cua and ~/.cua/envs/*),
# it:
#   1. unregisters the machine and the Spaces it provides from the relay,
#      deletes the service and the host's tokens (`cua host remove
#      --uninstalling`), after turning the provided Spaces off;
#   2. stops the cua daemon;
#   3. removes launch at login and the `cua` copy the app put on PATH.
# The user's data (~/.cua, the Spaces, settings) stays, as with the macOS
# and Windows uninstalls. Every step is best effort and bounded: a failure
# is said on stderr and never blocks the removal.

case "${1:-}" in
remove | purge) ;;
*) exit 0 ;;
esac

APP="/opt/Cua Spaces"
CUA="$APP/resources/native/cua"

say() { echo "cua-spaces: $*" >&2; }

# As user <id> (a name, a uid) with the session variables that reach their
# systemd user manager, for at most <seconds>:
#   as_user <seconds> <name> <uid> <home> <command...>
as_user() {
  secs=$1
  u=$2
  id=$3
  h=$4
  shift 4
  rt="/run/user/$id"
  limit=""
  command -v timeout >/dev/null 2>&1 && limit="timeout $secs"
  if [ "$id" = 0 ]; then
    $limit env HOME="$h" XDG_RUNTIME_DIR="$rt" DBUS_SESSION_BUS_ADDRESS="unix:path=$rt/bus" "$@"
  elif command -v runuser >/dev/null 2>&1; then
    $limit runuser -u "$u" -- env HOME="$h" XDG_RUNTIME_DIR="$rt" DBUS_SESSION_BUS_ADDRESS="unix:path=$rt/bus" "$@"
  else
    return 1
  fi
}

# The app first: a running one would start the daemon again.
quit_app() {
  pkill -TERM -f "^$APP/cua-spaces" 2>/dev/null || return 0
  n=0
  while pgrep -f "^$APP/cua-spaces" >/dev/null 2>&1 && [ "$n" -lt 10 ]; do
    sleep 0.5
    n=$((n + 1))
  done
  pkill -KILL -f "^$APP/cua-spaces" 2>/dev/null || true
}

# One cua home of one user: <name> <uid> <home> <cua home>.
turn_off() {
  u=$1
  id=$2
  h=$3
  ch=$4
  if [ -f "$ch/host/config.json" ]; then
    say "turning off unattended access ($ch)"
    if ! as_user 150 "$u" "$id" "$h" env CUA_HOME="$ch" "$CUA" host remove --force --uninstalling; then
      say "could not tell the relay this machine is gone; remove it from Machines in the app or at cua.ai"
    fi
  fi
  if [ -d "$ch" ]; then
    as_user 30 "$u" "$id" "$h" env CUA_HOME="$ch" "$CUA" daemon stop >/dev/null 2>&1 || true
  fi
  # Whatever the removal above could not reach (no session bus for a user
  # who is logged out): the unit file, and a driver still running.
  unit="$h/.config/systemd/user/cua-spacesd-host.service"
  if [ -f "$unit" ] && [ ! -f "$ch/host/config.json" ]; then
    as_user 20 "$u" "$id" "$h" systemctl --user disable --now cua-spacesd-host >/dev/null 2>&1 || true
  fi
  pkill -KILL -u "$id" -f "$ch/host/bin/" 2>/dev/null || true
}

# What the app wrote into one user's home.
clean_home() {
  u=$1
  id=$2
  h=$3
  autostart="$h/.config/autostart/cua-spaces.desktop"
  if [ -f "$autostart" ] && grep -q "cua-spaces" "$autostart" 2>/dev/null; then
    rm -f "$autostart"
  fi
  shim="$h/.local/bin/cua"
  if [ -e "$shim" ] || [ -L "$shim" ]; then
    if [ -L "$shim" ]; then
      case "$(readlink "$shim")" in "$APP"/*) rm -f "$shim" ;; esac
    elif [ -f "$CUA" ] && cmp -s "$shim" "$CUA" 2>/dev/null; then
      rm -f "$shim"
    fi
  fi
  # The copy of cua the daemon runs from when it cannot run from the package.
  data="${XDG_DATA_HOME:-$h/.local/share}/cua-spaces/daemon"
  [ -d "$data" ] && rm -rf "$data"
  pkill -KILL -u "$id" -f "cua-spaces/daemon/.*/cua daemon" 2>/dev/null || true
}

quit_app

getent passwd 2>/dev/null | while IFS=: read -r name _ id _ _ home _; do
  case "$id" in '' | *[!0-9]*) continue ;; esac
  # Real accounts only: root and ordinary users.
  [ "$id" = 0 ] || [ "$id" -ge 1000 ] || continue
  [ -d "$home" ] || continue
  if [ -x "$CUA" ]; then
    for ch in "$home/.cua" "$home"/.cua/envs/*; do
      [ -d "$ch" ] || continue
      turn_off "$name" "$id" "$home" "$ch"
    done
  fi
  clean_home "$name" "$id" "$home"
done

# Anything still running from the package (a daemon that did not answer).
pkill -KILL -f "^$APP/" 2>/dev/null || true

exit 0
