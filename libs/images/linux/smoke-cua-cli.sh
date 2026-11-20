#!/usr/bin/env bash
# Smoke test for the cua CLI in linux (container rootfs variant), under runc or runsc (gVisor):
# the slim tier, and so every tier, lets an agent in a Space make child Spaces.
#
# Offline-safe: no registry, no account, no host. It checks, as the desktop user (the agent):
#   health     the container HEALTHCHECK passes and cua-spacesd's own check script answers
#   version    `cua --version` runs, from /usr/local/bin, on the user's PATH
#   spawn      `cua spaces ls` inside a Space that has a spawn channel goes to that channel:
#              the Space's channel is CUA_SPAWN + CUA_SPAWN_TOKEN_FILE (what cua-spacesd sets
#              for processes it starts once the host opened the channel); here a listener
#              stands in for it and must receive the CLI's HTTP/2 (gRPC) connection
#   no channel without a channel the CLI does not try one: `cua spaces ls` never connects
#
# Usage: smoke-cua-cli.sh [--image REF] [--runtime runc|runsc] [--evidence DIR] [--keep]
set -euo pipefail
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
IMAGE="cua-e2e-local/linux:docker-local-$(host_arch)"
RUNTIME=runc
EVIDENCE="${SMOKE_EVIDENCE:-$PWD/smoke-evidence}"
KEEP=0
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --runtime) RUNTIME="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
mkdir -p "$EVIDENCE"
NAME="cua-e2e-cua-cli-$RUNTIME-$$"
PASS=0; FAIL=0
t_start=$SECONDS

ok() { PASS=$((PASS + 1)); echo "  ok   $*"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL $*"; }
check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then ok "$d"; else bad "$d"; fi; }
cleanup() {
    docker exec "$NAME" sh -c 'tail -n 60 /var/log/supervisor/cua-spacesd.log' \
        >"$EVIDENCE/$RUNTIME-cua-cli-logs.txt" 2>&1 || true
    [ "$KEEP" = 1 ] || docker rm -f "$NAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT
# Everything below runs as the desktop user, the way an agent's shell does.
as_cua() { docker exec -u cua -e CUA_TELEMETRY=0 "$NAME" "$@"; }

echo "==> $IMAGE under --runtime=$RUNTIME (cua CLI)"
docker run -d --name "$NAME" --runtime="$RUNTIME" --shm-size=512m \
    --memory="${SMOKE_MEMORY:-4g}" --memory-swap="${SMOKE_MEMORY:-4g}" \
    -e CUA_AUDIO=false -e CUA_ENV_TOKEN="smoke-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')" \
    "$IMAGE" >/dev/null

status=""
for _ in $(seq 1 120); do
    status="$(docker inspect -f '{{.State.Health.Status}}' "$NAME" 2>/dev/null || echo gone)"
    [ "$status" = healthy ] || [ "$status" = gone ] && break
    sleep 1
done
[ "$status" = healthy ] && ok "container healthy after $((SECONDS - t_start))s (HEALTHCHECK)" || { bad "container health=$status"; exit 1; }
check "cua-spacesd health check script answers" docker exec "$NAME" /opt/cua/healthcheck.d/10-spacesd.sh

echo "==> cua --version"
check "cua is /usr/local/bin/cua on the desktop user's PATH" bash -c "[ \"\$(docker exec -u cua $NAME sh -c 'command -v cua')\" = /usr/local/bin/cua ]"
ver="$(as_cua cua --version 2>&1 | head -1 || true)"
echo "    $ver"
grep -Eq '^cua [0-9]+\.[0-9]+\.[0-9]+' <<<"$ver" && ok "cua --version: $ver" || bad "cua --version: '$ver'"
check "cua links against nothing missing" bash -c "! docker exec $NAME ldd /usr/local/bin/cua | grep -q 'not found'"
echo "    source: $(docker exec "$NAME" cat /etc/cua-image/cua-cli-source 2>/dev/null) $(docker exec "$NAME" cat /etc/cua-image/cua-cli-release 2>/dev/null || true)"

echo "==> cua spaces ls reaches the Space's spawn channel"
PROBE_PORT=18999
probe() { # probe <label> <env...>: listen on the port, run the CLI, print the bytes the listener got
    local label="$1"; shift
    docker exec "$NAME" rm -f "/tmp/probe-$label"
    docker exec -d -u cua "$NAME" sh -c "timeout 25 nc -l 127.0.0.1 $PROBE_PORT >/tmp/probe-$label 2>/dev/null"
    sleep 1
    docker exec -u cua -e CUA_TELEMETRY=0 -e CUA_HOME=/tmp/cua-home-$label "$@" "$NAME" sh -c 'timeout 12 cua spaces ls >/tmp/ls-out 2>&1; echo $?' >"$EVIDENCE/$RUNTIME-cua-cli-$label-rc.txt" || true
    sleep 1
    docker exec "$NAME" sh -c "head -c 64 /tmp/probe-$label | od -An -c | head -2; cp /tmp/ls-out /tmp/ls-$label 2>/dev/null; true"
    docker exec "$NAME" sh -c "wc -c </tmp/probe-$label"
}
docker exec -u cua "$NAME" sh -c 'umask 077; echo smoke-capability >/tmp/spawn-token-probe'
got="$(probe channel -e CUA_SPAWN=http://127.0.0.1:$PROBE_PORT -e CUA_SPAWN_TOKEN_FILE=/tmp/spawn-token-probe | tail -1 | tr -d ' \r')"
[ "${got:-0}" -gt 0 ] && ok "with a spawn channel, cua spaces ls connected to it ($got bytes)" || bad "with a spawn channel, cua spaces ls never reached it"
docker exec "$NAME" cat /tmp/ls-channel >"$EVIDENCE/$RUNTIME-cua-cli-channel-out.txt" 2>&1 || true
grep -q "PRI \* HTTP/2" < <(docker exec "$NAME" sh -c 'head -c 24 /tmp/probe-channel') && ok "the connection is gRPC over HTTP/2 (the spawn channel's protocol)" || bad "the bytes at the spawn address are not HTTP/2"

got="$(probe none | tail -1 | tr -d ' \r')"
[ "${got:-0}" = 0 ] && ok "without a spawn channel, cua spaces ls does not connect to one" || bad "without a spawn channel, something connected ($got bytes)"

echo "==> $PASS passed, $FAIL failed in $((SECONDS - t_start))s ($RUNTIME, $IMAGE)"
[ "$FAIL" = 0 ]
