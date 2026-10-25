#!/usr/bin/env bash
# Real-build check of the Space list at scale, inside the builder Space:
# a real cua-relay (mesh on), N real cua-spacesd computers (`join`, headless),
# the real `cua daemon` (throwaway CUA_HOME) and list_probe doing what the
# apps do. Then K computers switched off (killed) and H asleep (SIGSTOP:
# still connected as far as the relay knows, answering nothing).
#
#   run.sh LABEL [N K H POLLS INTERVAL_MS DEADLINE_MS]
#
# Run from libs/cua. Prints the probe's JSON lines and one SAMPLE line per
# 5 s with the daemon's open files, threads and memory.
set -u
LABEL=${1:?label}; N=${2:-20}; K=${3:-5}; H=${4:-3}
POLLS=${5:-7}; INTERVAL=${6:-10000}; DEADLINE=${7:-4000}
export CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_DEBUG=0
cargo build -q --locked --release -p cua-cli -p cua-sdk --example list_probe || exit 3
(cd ../cua-spacesd && cargo build -q --locked --release -p cua-relay) || exit 3
B=${CARGO_TARGET_DIR:-/target}/release
SPD=$(cd ../.. && pwd)/.e2e/cua-spacesd
W=$(mktemp -d)
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill -CONT "$p" 2>/dev/null; kill "$p" 2>/dev/null; done
  CUA_HOME=$W/home "$B/cua" daemon stop >/dev/null 2>&1
  wait 2>/dev/null; rm -rf "$W"
}
trap cleanup EXIT
PORT=18480
CUA_RELAY_LISTEN=127.0.0.1:$PORT CUA_RELAY_TOKENS=tok-a,tok-b,tok-c CUA_RELAY_MESH=on \
  CUA_RELAY_ADMIN_TOKEN=adm CUA_RELAY_MESH_STUN_LISTEN=127.0.0.1:18478 \
  CUA_RELAY_STATE_FILE=$W/state.json CUA_RELAY_PUBLIC_URL=http://127.0.0.1:$PORT \
  CUA_RELAY_RECONNECT_GRACE_SECS=0 CUA_RELAY_MAX_MACHINES_PER_TOKEN=16 \
  "$B/cua-relay" > "$W/relay.log" 2>&1 & PIDS+=($!)
for _ in $(seq 100); do curl -sf http://127.0.0.1:$PORT/healthz >/dev/null && break; sleep 0.2; done
toks=(tok-a tok-b tok-c); MP=()
for i in $(seq 1 "$N"); do
  id=$(printf 'e2e-m-%03d' "$i")
  "$SPD" join --relay http://127.0.0.1:$PORT --relay-token "${toks[$(( (i-1) / 15 ))]}" \
    --machine-id "$id" --token "env-$id" --no-desktop --no-driver --no-mcp --no-access-log \
    --listen 127.0.0.1:$((21000+i)) --quic-port 0 --data-dir "$W/m/$id" \
    > "$W/m-$id.log" 2>&1 & p=$!; PIDS+=($p); MP+=($p)
done
for _ in $(seq 150); do
  n=$(curl -sf -H 'authorization: Bearer adm' http://127.0.0.1:$PORT/relay/v1/machines | grep -o '"online":true' | wc -l)
  [ "$n" -ge "$N" ] && break; sleep 0.2
done
echo "SETUP machines_online=$n"
export CUA_HOME=$W/home
"$B/cua" daemon start >/dev/null 2>&1
for i in $(seq 1 "$N"); do
  id=$(printf 'e2e-m-%03d' "$i")
  "$B/cua" spaces add "http://127.0.0.1:$PORT/m/$id" --token "env-$id" --name "$id" >/dev/null 2>&1 \
    || echo "SETUP add failed: $id"
done
DPID=$(pgrep -f "$B/cua daemon" | head -1)
echo "SETUP daemon=$DPID spaces=$("$B/cua" spaces ls 2>/dev/null | grep -c e2e-m-)"
# Restart the daemon so the probe starts cold, as after an update.
"$B/cua" daemon stop >/dev/null 2>&1; sleep 1
# K switched off, H asleep.
for j in $(seq 0 $((K-1))); do kill -9 "${MP[$j]}"; done
for j in $(seq "$K" $((K+H-1))); do kill -STOP "${MP[$j]}"; done
sleep 3
"$B/cua" daemon start >/dev/null 2>&1
DPID=$(pgrep -f "$B/cua daemon" | head -1)
( while kill -0 "$DPID" 2>/dev/null; do
    fds=$(ls /proc/$DPID/fd 2>/dev/null | wc -l)
    thr=$(ls /proc/$DPID/task 2>/dev/null | wc -l)
    rss=$(awk '/VmRSS/{print $2}' /proc/$DPID/status 2>/dev/null)
    echo "SAMPLE label=$LABEL t=$SECONDS fds=$fds threads=$thr rss_kb=$rss"
    sleep 5
  done ) & PIDS+=($!)
"$B/examples/list_probe" --polls "$POLLS" --interval-ms "$INTERVAL" --deadline-ms "$DEADLINE" --label "$LABEL"
