#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Start / stop a local Spaces container (linux) for the streaming
# examples and benchmarks. One container at a time, 4 GiB, loopback ports.
#
#   space.sh start [--runtime runc|runsc] [--image REF] [--name NAME]
#   space.sh stop  [--name NAME]
#   space.sh env   [--name NAME]      # print CUA_ENV_URL / CUA_ENV_TOKEN exports
#   space.sh fixture MODE [--name NAME]   # (re)start the bench fixture
#
# Container names are unique per run: `start` without --name (or
# CUA_BENCH_CONTAINER) picks cua-e2e-bench-space-<pid>-<time> and records it,
# so later commands without --name act on the last container started here.
# The script only ever removes a container it created (by the ID it recorded
# at start); it never force-removes a foreign container that shares a name.
#
# CUA_BENCH_DRIVER_BIN=<linux cua-spacesd>  replaces the image's driver
# after start (benchmark the driver built from this commit).
#
# Ports (host loopback): 33211 -> 3211 (gRPC + /media), 33212/udp -> 3212
# (QUIC media), 33281 -> 18081 (bench fixture time server). Override with
# CUA_BENCH_PORT_BASE (default 33200: +11, +12, +81).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
IMAGE="${CUA_BENCH_IMAGE:-cua-e2e-local/linux:docker-local-$(host_arch)}"
RUNTIME=runc
NAME="${CUA_BENCH_CONTAINER:-}"
STATE_DIR="${TMPDIR:-/tmp}"
LAST_FILE="$STATE_DIR/cua-e2e-bench-space.last"
BASE="${CUA_BENCH_PORT_BASE:-33200}"
cmd="${1:-}"; shift || true
MODE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --runtime) RUNTIME="$2"; shift 2 ;;
        --image) IMAGE="$2"; shift 2 ;;
        --name) NAME="$2"; shift 2 ;;
        *) MODE="$1"; shift ;;
    esac
done

if [ -z "$NAME" ]; then
    if [ "$cmd" = start ]; then
        NAME="cua-e2e-bench-space-$$-$(date +%s)"
    elif [ -r "$LAST_FILE" ]; then
        NAME="$(cat "$LAST_FILE")"
    else
        echo "no --name given and no container started by space.sh" >&2
        exit 2
    fi
fi

TOKEN_FILE="$STATE_DIR/$NAME.token"
# The ID of the container this script created under $NAME.
CID_FILE="$STATE_DIR/$NAME.cid"
owned_cid() { [ -r "$CID_FILE" ] && cat "$CID_FILE"; }
case "$cmd" in
start)
    if existing="$(docker container inspect -f '{{.Id}}' "$NAME" 2>/dev/null)"; then
        if [ -n "$existing" ] && [ "$existing" = "$(owned_cid || true)" ]; then
            # A leftover from an earlier start of ours: safe to replace.
            docker rm -f "$existing" >/dev/null
        else
            echo "a container named $NAME already exists and was not created by space.sh; not touching it" >&2
            exit 1
        fi
    fi
    token="bench-$(date +%s)-$RANDOM$RANDOM"
    printf '%s' "$token" >"$TOKEN_FILE"
    cid="$(docker run -d --name "$NAME" --runtime="$RUNTIME" --shm-size=512m \
        --memory=4g --memory-swap=4g \
        --label org.trycua.bench=streaming \
        -e CUA_ENV_TOKEN="$token" -e CUA_ENV_QUIC_PORT=3212 \
        -p "127.0.0.1:$((BASE + 11)):3211" -p "127.0.0.1:$((BASE + 12)):3212/udp" \
        -p "127.0.0.1:$((BASE + 81)):18081" \
        "$IMAGE")"
    printf '%s' "$cid" >"$CID_FILE"
    printf '%s' "$NAME" >"$LAST_FILE"
    for _ in $(seq 1 90); do
        s="$(docker inspect -f '{{.State.Health.Status}}' "$NAME" 2>/dev/null || echo gone)"
        [ "$s" = healthy ] && break
        [ "$s" = gone ] && { echo "container exited" >&2; exit 1; }
        sleep 1
    done
    [ "$s" = healthy ] || { echo "container not healthy ($s)" >&2; exit 1; }
    # The driver may take a moment after the desktop is healthy.
    for _ in $(seq 1 30); do
        nc -z 127.0.0.1 "$((BASE + 11))" 2>/dev/null && break
        sleep 1
    done
    # Optional driver override (e.g. a freshly built cua-spacesd for this
    # commit): copy it over the image's binary and restart the program.
    if [ -n "${CUA_BENCH_DRIVER_BIN:-}" ]; then
        # Through `docker exec` stdin, not `docker cp`: under gVisor (runsc)
        # the rootfs overlay lives in the sandbox, so `docker cp` writes a
        # layer the guest never sees and the image's driver kept running.
        # Write beside the running binary and rename over it (text busy).
        docker exec -i "$NAME" sh -c 'cat > /usr/local/bin/cua-spacesd.new && chmod 0755 /usr/local/bin/cua-spacesd.new && mv -f /usr/local/bin/cua-spacesd.new /usr/local/bin/cua-spacesd' <"$CUA_BENCH_DRIVER_BIN"
        docker exec "$NAME" supervisorctl restart cua-spacesd >/dev/null
        want="$(shasum -a 256 "$CUA_BENCH_DRIVER_BIN" 2>/dev/null || sha256sum "$CUA_BENCH_DRIVER_BIN")"
        got="$(docker exec "$NAME" sha256sum /usr/local/bin/cua-spacesd)"
        [ "${want%% *}" = "${got%% *}" ] || { echo "driver override did not reach the guest" >&2; exit 1; }
        sleep 1
        for _ in $(seq 1 30); do
            nc -z 127.0.0.1 "$((BASE + 11))" 2>/dev/null && break
            sleep 1
        done
        echo "driver replaced with $CUA_BENCH_DRIVER_BIN"
    fi
    echo "started $NAME ($RUNTIME) env=http://127.0.0.1:$((BASE + 11))"
    ;;
stop)
    cid="$(owned_cid || true)"
    if [ -z "$cid" ]; then
        echo "$NAME was not started by space.sh; not removing it"
    elif docker rm -f "$cid" >/dev/null 2>&1; then
        echo "removed $NAME"
    else
        echo "$NAME not running"
    fi
    rm -f "$CID_FILE" "$TOKEN_FILE"
    if [ "$(cat "$LAST_FILE" 2>/dev/null)" = "$NAME" ]; then rm -f "$LAST_FILE"; fi
    ;;
env)
    echo "export CUA_ENV_URL=http://127.0.0.1:$((BASE + 11))"
    echo "export CUA_ENV_TOKEN=$(cat "$TOKEN_FILE")"
    echo "export CUA_ENV_QUIC_ADDR=127.0.0.1:$((BASE + 12))"
    ;;
fixture)
    MODE="${MODE:-timecode}"
    docker cp "$HERE/../fixtures/benchfix.py" "$NAME:/tmp/benchfix.py"
    docker exec "$NAME" sh -c 'pkill -f "benchfix[.]py"; [ -r /tmp/benchfix.pid ] && kill "$(cat /tmp/benchfix.pid)"; true' 2>/dev/null
    docker exec -d -u cua "$NAME" bash -c \
        "set -a; . /run/cua-desktop/desktop.env; set +a; echo \$\$ > /tmp/benchfix.pid; exec python3 /tmp/benchfix.py --mode $MODE >/tmp/benchfix.out 2>&1"
    for _ in $(seq 1 50); do
        docker exec "$NAME" sh -c "grep -q '\"mode\": \"$MODE\"' /tmp/cua-fixtures/benchfix.jsonl 2>/dev/null" && break
        sleep 0.2
    done
    echo "fixture $MODE running in $NAME"
    ;;
*)
    echo "usage: space.sh start|stop|env|fixture [MODE] [--runtime runc|runsc] [--image REF] [--name NAME]" >&2
    exit 2
    ;;
esac
