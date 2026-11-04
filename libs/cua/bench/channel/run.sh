#!/usr/bin/env bash
# The coordinator-to-Space channel, measured and checked against a baseline.
#
#   CUA=/path/to/cua BENCH=/path/to/channel_bench bash run.sh BASELINE [SPACE]
#
# With SPACE (an id or name the running daemon resolves) it measures that
# Space: a LAN run names a Space one of your other machines provides
# (`cua spaces create linux --on host:<machine>`). Without it, it creates a
# Linux VM Space on this machine (`--kind vm`, 2 vCPU, 2 GiB), measures it
# and deletes it. Then `check.py BASELINE` decides (exit 1 on a regression).
#
# Environment: CUA (default `cua`), BENCH (default `channel_bench`), OUT
# (default ./results), FACTOR (default 1.5), ITERS (default 300), FILES
# (default `1:20,100:5`), IMAGE (default `linux`). RECORD=1 writes the run as
# a new baseline (BASELINE.new.json next to the output) instead of checking.
# OVERLAY_SPACESD (a Linux cua-spacesd built from this tree) and UDEV_RULE
# (the image's 71-cua-bulk-lane.rules) put the guest end of the bulk lane into
# a Space made from an older published image; the daemon then reconnects so
# its client finds the lane.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
baseline=${1:?baseline JSON}
space=${2:-}
CUA=${CUA:-cua}
BENCH=${BENCH:-channel_bench}
OUT=${OUT:-results}
FACTOR=${FACTOR:-1.5}
mkdir -p "$OUT"
export DO_NOT_TRACK=1 CUA_TELEMETRY=0
created=
cleanup() { [ -n "$created" ] && "$CUA" spaces delete "$created" --force >/dev/null 2>&1 || true; }
trap cleanup EXIT
if [ -z "$space" ]; then
  created=channel-bench-$$
  "$CUA" spaces create "${IMAGE:-linux}" --on local --kind vm --cpus 2 --memory-mb 2048 --name "$created" >/dev/null
  space=$created
  if [ -n "${OVERLAY_SPACESD:-}" ]; then
    "$CUA" sb overlay "$created" "cua-spacesd=$OVERLAY_SPACESD" >/dev/null
    if [ -n "${UDEV_RULE:-}" ]; then
      "$CUA" sb cp "$UDEV_RULE" "$created:/tmp/71-cua-bulk-lane.rules" >/dev/null
      "$CUA" sb exec "$created" "sudo cp /tmp/71-cua-bulk-lane.rules /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=block --action=change && sleep 1" >/dev/null
    fi
    "$CUA" daemon stop >/dev/null && "$CUA" daemon start >/dev/null
  fi
fi
out="$OUT/$(basename "$baseline" .json)-$(date -u +%Y%m%dT%H%M%SZ).jsonl"
"$BENCH" --space "$space" --iters "${ITERS:-300}" --files "${FILES:-1:20,100:5}" --label "$(basename "$baseline" .json)" | tee "$out"
if [ "${RECORD:-0}" = 1 ] || [ ! -f "$baseline" ]; then
  python3 "$here/check.py" --record "$out" > "$OUT/$(basename "$baseline" .json).new.json"
  echo "recorded $OUT/$(basename "$baseline" .json).new.json"
  # No baseline yet: nothing was checked, which is a failure unless asked.
  [ -f "$baseline" ] || { echo "no baseline $baseline: commit the recorded one" >&2; exit 1; }
  exit 0
fi
python3 "$here/check.py" "$baseline" "$out" --factor "$FACTOR"
