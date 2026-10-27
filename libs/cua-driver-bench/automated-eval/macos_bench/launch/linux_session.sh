#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Usage: linux_session.sh [--evidence DIR] [--check-run DIR]... -- COMMAND [ARG...]
#
# Runs COMMAND inside a headless X11 session (Xvfb 1920x1080x24, private D-Bus,
# openbox, picom/xrender). Evidence (launch.log, component logs, at-spi-address.txt,
# pids.jsonl, exit) goes to a directory outside any git checkout. Only processes this
# script started are stopped. COMMAND's exit status passes through; a leaked process
# or incomplete --check-run evidence turns a zero status into 1.
set -uo pipefail

BENCH="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
usage() {
  echo "usage: ${0##*/} [--evidence DIR] [--check-run DIR]... -- COMMAND [ARG...]" >&2
  exit 2
}

[ "$(uname -s)" = Linux ] || { echo "linux_session.sh requires Linux" >&2; exit 2; }

evidence=""
checks=()
while [ $# -gt 0 ]; do
  case "$1" in
    --evidence) [ $# -ge 2 ] || usage; evidence="$2"; shift 2 ;;
    --check-run) [ $# -ge 2 ] || usage; checks+=("$2"); shift 2 ;;
    --) shift; break ;;
    *) usage ;;
  esac
done
[ $# -gt 0 ] || usage

new=0
if [ -z "$evidence" ]; then
  root="${CDB_BENCH_WORK:-$HOME/.cache/cua-bench-h2h}/linux-sessions"
  mkdir -p "$root" && evidence="$(mktemp -d "$root/run-XXXXXXXX")" || exit 2
  new=1
elif [ ! -e "$evidence" ]; then
  mkdir -m 0700 -p "$evidence" || exit 2
  new=1
fi
evidence="$(cd "$evidence" && pwd -P)" || exit 2
if git -C "$evidence" rev-parse --show-toplevel >/dev/null 2>&1; then
  echo "evidence directory must be outside a git checkout: $evidence" >&2
  exit 2
fi
chmod 0700 "$evidence"

export CDB_SESSION_DIR="$evidence"
export CDB_BENCH_PID_LOG="$evidence/pids.jsonl"
[ "$new" = 1 ] && : > "$CDB_BENCH_PID_LOG"

inner='
unset WAYLAND_DISPLAY
export XDG_SESSION_TYPE=x11
owned=()
cleanup() {
  for p in "${owned[@]}"; do kill "$p" 2>/dev/null || true; done
  for p in "${owned[@]}"; do wait "$p" 2>/dev/null || true; done
}
trap cleanup EXIT
trap "exit 143" TERM
trap "exit 130" INT
openbox >> "$CDB_SESSION_DIR/openbox.log" 2>&1 &
owned+=("$!")
printf "{\"pid\":%s,\"role\":\"openbox\"}\n" "$!" >> "$CDB_BENCH_PID_LOG"
picom --backend xrender --config /dev/null >> "$CDB_SESSION_DIR/picom.log" 2>&1 &
owned+=("$!")
printf "{\"pid\":%s,\"role\":\"picom\"}\n" "$!" >> "$CDB_BENCH_PID_LOG"
timeout 15 bash -c "until xprop -root _NET_SUPPORTING_WM_CHECK | grep -q \"window id\"; do sleep 0.2; done"
for p in "${owned[@]}"; do kill -0 "$p" 2>/dev/null || { echo "session component $p exited early" >&2; exit 1; }; done
timeout 15 gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus --method org.a11y.Bus.GetAddress > "$CDB_SESSION_DIR/at-spi-address.txt"
"$@"
'

timeout "${CDB_SESSION_TIMEOUT:-0}" xvfb-run -a --server-args='-screen 0 1920x1080x24' \
  dbus-run-session -- bash -euo pipefail -c "$inner" linux-session "$@" 2>&1 \
  | tee -a "$CDB_SESSION_DIR/launch.log"
status=${PIPESTATUS[0]}
command_status=$status

evidence_status=0
python3 "$BENCH/tools/check_run_evidence.py" --ledger-only "$CDB_BENCH_PID_LOG" || evidence_status=1
for run in "${checks[@]}"; do
  python3 "$BENCH/tools/check_run_evidence.py" "$run" || evidence_status=1
done
if [ "$evidence_status" -ne 0 ] && [ "$status" -eq 0 ]; then
  status=1
fi
echo "command=$command_status evidence=$evidence_status" > "$CDB_SESSION_DIR/exit"
echo "evidence: $CDB_SESSION_DIR"
exit "$status"
