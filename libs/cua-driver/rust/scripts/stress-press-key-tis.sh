#!/bin/bash
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Stress press_key and hotkey against a fresh Cua Driver daemon on macOS,
# with optional keyboard-layout switches, and report whether the daemon
# survived (CUA-3763: Text Input Sources called off the main thread aborted
# or trapped the daemon).
#
# Run it from Terminal.app in a GUI session, not over ssh: the daemon must run
# its AppKit event loop in the user's session for the crash to be possible.
# Terminal needs Accessibility (the daemon is its child), or the app does with
# DAEMON_LAUNCH=open. The keys go to a Terminal window by default
# (TARGET_APP=TextEdit opens a scratch document instead).
#
# Before the fix, on macOS 26.5.2 the daemon traps (SIGTRAP) on the first
# press_key; the RESULT line then reports daemon_alive=no.
#
#   stress-press-key-tis.sh <path/to/CuaDriver.app/Contents/MacOS/cua-driver> \
#       [calls=500] [concurrency=8] [tis_stress binary for layout switches] \
#       [layout id=com.apple.keylayout.French]
#
# The layout switcher is the platform-macos example:
#   cargo build --release -p platform-macos --example tis_stress
set -u
BIN=${1:?usage: $0 <cua-driver> [calls] [concurrency] [tis_stress] [layout]}
CALLS=${2:-500}
CONC=${3:-8}
SWITCHER=${4:-}
LAYOUT=${5:-com.apple.keylayout.French}
MODE=${DELIVERY_MODE:-background}
SOCK=$(mktemp -u /tmp/cua-tis-stress.XXXXXX).sock
WORK=$(mktemp -d /tmp/cua-tis-stress.XXXXXX)
touch "$WORK/start"

# The daemon runs as a direct child of this shell, as an MCP client or the
# Bench harness starts it (Accessibility is then attributed to the terminal).
# On macOS 26.5 that is what makes HIToolbox rebuild its input-source list on
# the first off-main lookup and trap. DAEMON_LAUNCH=open starts it through
# LaunchServices instead, so the app's own Accessibility grant applies.
if [ "${DAEMON_LAUNCH:-direct}" = open ]; then
  APP=${BIN%/Contents/MacOS/*}
  open -n -g "$APP" --stdout "$WORK/daemon.log" --stderr "$WORK/daemon.log" \
    --args serve --socket "$SOCK" --no-permissions-gate
else
  "$BIN" serve --socket "$SOCK" --no-permissions-gate >"$WORK/daemon.log" 2>&1 &
fi
for _ in $(seq 50); do [ -S "$SOCK" ] && break; sleep 0.2; done
DAEMON=$(pgrep -f "serve --socket $SOCK" | head -1)
# The target app: TextEdit with a scratch document, or $TARGET_APP (for
# example Terminal, the app running this script).
TARGET_APP=${TARGET_APP:-Terminal}
if [ "$TARGET_APP" = TextEdit ]; then
  echo "stress target" >"$WORK/target.txt"
  open -e "$WORK/target.txt" && sleep 3
fi
TARGET=$(ps -axo pid=,comm= | awk -v app="/$TARGET_APP.app/Contents/MacOS/$TARGET_APP" 'index($0, app) {print $1; exit}')
WINDOW=$("$BIN" call --socket "$SOCK" list_windows "{\"pid\":$TARGET}" 2>/dev/null |
  grep -o '"window_id": *[0-9]*' | head -1 | grep -o '[0-9]*$')
echo "daemon=$DAEMON target=$TARGET window=$WINDOW calls=$CALLS concurrency=$CONC switcher=${SWITCHER:-none}"

if [ -n "$SWITCHER" ]; then
  # Switch layouts every 250 ms for as long as the calls run.
  "$SWITCHER" --switch-only --switch "$LAYOUT" --switch-count 1000000 --interval-ms 250 \
    --until-file "$WORK/calls-done" >"$WORK/switch.log" 2>&1 &
  SWITCH=$!
fi

call() {
  local i=$1
  if [ $((i % 2)) -eq 0 ]; then
    "$BIN" call --socket "$SOCK" press_key "{\"pid\":$TARGET,\"window_id\":$WINDOW,\"key\":\"a\",\"modifiers\":[\"cmd\"],\"delivery_mode\":\"$MODE\"}"
  else
    "$BIN" call --socket "$SOCK" hotkey "{\"pid\":$TARGET,\"window_id\":$WINDOW,\"keys\":[\"ctrl\",\"e\"],\"delivery_mode\":\"$MODE\"}"
  fi >"$WORK/call-$i.log" 2>&1 && echo ok || echo err
}
export -f call
export BIN SOCK TARGET WINDOW WORK MODE
seq "$CALLS" | xargs -P "$CONC" -I{} bash -c 'call {}' >"$WORK/results.txt"
touch "$WORK/calls-done"

[ -n "${SWITCH:-}" ] && wait "$SWITCH" 2>/dev/null
OK=$(grep -c '^ok$' "$WORK/results.txt")
ERR=$(grep -c '^err$' "$WORK/results.txt")
if kill -0 "$DAEMON" 2>/dev/null; then ALIVE=yes; else ALIVE=no; fi
CRASHES=$(find ~/Library/Logs/DiagnosticReports -name 'cua-driver*.ips' -newer "$WORK/start" 2>/dev/null | wc -l | tr -d ' ')
SWITCHES=$(grep -o 'switches=[0-9]*' "$WORK/switch.log" 2>/dev/null | cut -d= -f2)
echo "RESULT calls=$CALLS ok=$OK err=$ERR daemon_alive=$ALIVE crash_reports=$CRASHES layout_switches=${SWITCHES:-0}"
kill "$DAEMON" 2>/dev/null
[ "$TARGET_APP" = TextEdit ] && osascript -e 'tell application "TextEdit" to quit saving no' >/dev/null 2>&1
rm -f "$SOCK"
FIRST_ERR=$(grep -l . "$WORK"/call-*.log 2>/dev/null | head -1)
[ "$ERR" -gt 0 ] && [ -n "$FIRST_ERR" ] && echo "first error: $(head -c 300 "$FIRST_ERR")"
echo "logs: $WORK"
