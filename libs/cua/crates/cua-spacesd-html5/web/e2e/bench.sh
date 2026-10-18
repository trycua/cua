#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Click-to-pixel latency and CPU: the cua-spacesd HTML5 viewer (new image)
# against noVNC -> websockify -> Xvnc (an older image that still has them).
#
#   bench.sh --new REF --old REF [--out DIR] [--n 30]
#
# Opt-in (starts containers, 4 GiB caps); removes what it creates.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEB="$(cd "$HERE/.." && pwd)"
NEW=""; OLD=""; OUT="$PWD/viewer-bench"; N=30
PW_IMAGE="${PW_IMAGE:-mcr.microsoft.com/playwright:v1.63.0-noble}"
while [ $# -gt 0 ]; do
    case "$1" in
        --new) NEW="$2"; shift 2 ;;
        --old) OLD="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --n) N="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
mkdir -p "$OUT" "$HOME/.cache"
WORK="$(mktemp -d "$HOME/.cache/cua-e2e-webviewer-bench.XXXXXX")"
SBX="cua-e2e-webviewer-bench-$$"
cleanup() { docker rm -f "$SBX-pw" "$SBX" >/dev/null 2>&1 || true; rm -rf "$WORK"; }
trap cleanup EXIT
"$WEB/node_modules/.bin/esbuild" "$HERE/latency.bench.ts" --bundle --platform=node --format=esm \
    --external:playwright --outfile="$WORK/bench.mjs" --log-level=warning
cp "$HERE/probe.py" "$WORK/probe.py"

cpu_avg() { # container, samples -> mean CPU %
    local sum=0 n=0 v
    for _ in $(seq 1 "$2"); do
        v="$(docker stats --no-stream --format '{{.CPUPerc}}' "$1" | tr -d '%')"
        sum="$(echo "$sum + $v" | bc -l)"; n=$((n + 1))
    done
    echo "scale=1; $sum / $n" | bc -l
}

run_mode() { # mode image
    local mode="$1" image="$2" token="bench-$RANDOM$RANDOM"
    docker rm -f "$SBX" >/dev/null 2>&1 || true
    docker run -d --name "$SBX" --shm-size=512m --memory=4g --memory-swap=4g --cpus=4 \
        -e CUA_ENV_TOKEN="$token" -e CUA_NOVNC=true "$image" >/dev/null
    for _ in $(seq 1 90); do [ "$(docker inspect -f '{{.State.Health.Status}}' "$SBX")" = healthy ] && break; sleep 1; done
    docker cp "$WORK/probe.py" "$SBX:/tmp/probe.py"
    # Started through sh so its PID is recorded (and only that PID stopped).
    docker exec -u cua -d "$SBX" sh -c 'echo $$ >/tmp/probe.pid; exec desktop-env python3 /tmp/probe.py'
    sleep 3
    # 1. latency
    docker run --rm --name "$SBX-pw" --network "container:$SBX" --memory=4g --memory-swap=4g --cpus=4 --shm-size=1g \
        -e MODE="$mode" -e N="$N" -e OUT=/out -e CUA_ENV_TOKEN="$token" -v "$WORK:/work" -v "$OUT:/out" -w /work "$PW_IMAGE" \
        bash -c 'npm init -y >/dev/null && npm i --no-audit --no-fund --silent playwright@1.63.0 >/dev/null && timeout 600 node bench.mjs'
    # 2. CPU and bytes under motion: flat flips (30/s over the probe
    #    window), then a moving gradient scene (every pixel changes).
    for scene in 1 scene; do
        docker exec "$SBX" sh -c 'kill "$(cat /tmp/probe.pid)"' || true
        docker exec -u cua -d -e ANIM="$scene" "$SBX" sh -c 'echo $$ >/tmp/probe.pid; exec desktop-env python3 /tmp/probe.py'
        sleep 2
        docker run -d --name "$SBX-pw" --network "container:$SBX" --memory=4g --memory-swap=4g --cpus=4 --shm-size=1g \
            -e MODE="$mode" -e N=0 -e HOLD_SECONDS=40 -e OUT=/work/hold -e CUA_ENV_TOKEN="$token" -v "$WORK:/work" -w /work "$PW_IMAGE" \
            bash -c 'mkdir -p /work/hold && timeout 300 node bench.mjs' >/dev/null
        sleep 15
        local net0 guest browser net1 label
        net0="$(docker exec "$SBX" cat /proc/net/dev | awk '/lo:/{print $10}')"
        guest="$(cpu_avg "$SBX" 8)"
        browser="$(cpu_avg "$SBX-pw" 8)"
        net1="$(docker exec "$SBX" cat /proc/net/dev | awk '/lo:/{print $10}')"
        docker rm -f "$SBX-pw" >/dev/null 2>&1 || true
        label="$([ "$scene" = 1 ] && echo flat || echo scene)"
        printf '{"mode":"%s","content":"%s","guest_cpu_pct":%s,"browser_cpu_pct":%s,"loopback_tx_bytes_during_sampling":%s}\n' \
            "$mode" "$label" "$guest" "$browser" "$((net1 - net0))" | tee "$OUT/cpu-$mode-$label.json"
        sleep 5
    done
    docker rm -f "$SBX" >/dev/null 2>&1 || true
}

run_mode viewer "$NEW"
run_mode novnc "$OLD"
