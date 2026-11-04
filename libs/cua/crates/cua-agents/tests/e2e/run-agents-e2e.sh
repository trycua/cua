#!/usr/bin/env bash
# Live agent runs in a local container sandbox, against the scripted mock
# provider (cua-mock-llm). Starts, on a private docker network:
#   - the sandbox image (runc or runsc = gVisor), --memory=4g, spacesd on a
#     loopback port;
#   - cua-mock-llm in a debian:bookworm-slim container, --memory=256m;
# then runs `cargo test -p cua-agents --test e2e_live`. Both containers and
# the network are always removed.
#
#   run-agents-e2e.sh [--image REF] [--runtime runc|runsc] [--harnesses a,b]
#                     [--evidence DIR] [--keep]
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "$HERE/../../../.." && pwd)" # libs/cua
case "$(uname -m)" in arm64|aarch64) ARCH=aarch64 ;; *) ARCH=x86_64 ;; esac
IMAGE="ghcr.io/trycua/linux:24.04"
RUNTIME=runc
HARNESSES="claude-code"
EVIDENCE=""
KEEP=0
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --runtime) RUNTIME="$2"; shift 2 ;;
        --harnesses) HARNESSES="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
EVIDENCE="${EVIDENCE:-$(mktemp -d)}"
mkdir -p "$EVIDENCE"
ID="$$"
NET="cua-e2e-agents-net-$ID"
# gVisor's netstack cannot reach Docker's embedded DNS (127.0.0.11) on a
# user network here, so under runsc both containers use the default bridge
# (host resolvers) and the sandbox reaches the mock provider by IP.
[ "$RUNTIME" = runsc ] && NET=bridge
BOX="cua-e2e-agents-$ID"
MOCK="cua-e2e-mock-llm-$ID"
TOKEN="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
MOCK_KEY="mock-$(head -c 12 /dev/urandom | od -An -tx1 | tr -d ' \n')"
ENV_FILE="$(mktemp)"; chmod 600 "$ENV_FILE"
printf 'CUA_ENV_TOKEN=%s\n' "$TOKEN" >"$ENV_FILE"
MOCK_ENV="$(mktemp)"; chmod 600 "$MOCK_ENV"
printf 'CUA_MOCK_LLM_KEY=%s\n' "$MOCK_KEY" >"$MOCK_ENV"

cleanup() {
    rm -f "$ENV_FILE" "$MOCK_ENV"
    docker logs "$MOCK" >"$EVIDENCE/mock-llm.log" 2>&1 || true
    docker cp "$MOCK:/captures" "$EVIDENCE/captures" >/dev/null 2>&1 || true
    if [ "$KEEP" != 1 ]; then
        docker rm -f "$BOX" "$MOCK" >/dev/null 2>&1 || true
        [ "$NET" = bridge ] || docker network rm "$NET" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

echo "==> building cua-mock-llm ($ARCH-unknown-linux-musl)"
(cd "$WORKSPACE" && cargo zigbuild --release -q -p cua-mock-llm --target "$ARCH-unknown-linux-musl")
BIN="${CARGO_TARGET_DIR:-$WORKSPACE/target}/$ARCH-unknown-linux-musl/release/cua-mock-llm"
[ -f "$BIN" ] || { echo "cua-mock-llm was not built at $BIN" >&2; exit 1; }

[ "$NET" = bridge ] || docker network create "$NET" >/dev/null
alias=(--network-alias mock-llm); [ "$NET" = bridge ] && alias=()
docker run -d --name "$MOCK" --network "$NET" ${alias[@]+"${alias[@]}"} --memory=256m \
    --env-file "$MOCK_ENV" -v "$BIN:/usr/local/bin/cua-mock-llm:ro" debian:bookworm-slim \
    cua-mock-llm --listen 0.0.0.0:8787 --capture-dir /captures >/dev/null
echo "==> $IMAGE ($RUNTIME) as $BOX"
ENDPOINT="http://mock-llm:8787"
if [ "$NET" = bridge ]; then
    ENDPOINT="http://$(docker inspect -f '{{.NetworkSettings.Networks.bridge.IPAddress}}' "$MOCK"):8787"
fi
docker run -d --name "$BOX" --network "$NET" --runtime="$RUNTIME" --memory=4g --memory-swap=4g \
    --shm-size=512m --env-file "$ENV_FILE" -p 127.0.0.1::3211 "$IMAGE" >/dev/null
for _ in $(seq 1 180); do
    s="$(docker inspect -f '{{.State.Health.Status}}' "$BOX" 2>/dev/null || echo gone)"
    [ "$s" = healthy ] || [ "$s" = gone ] && break
    sleep 1
done
PORT="$(docker port "$BOX" 3211/tcp | head -1 | sed 's/.*://')"
for _ in $(seq 1 60); do
    [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/health")" = 204 ] && break
    sleep 1
done
echo "==> spacesd on 127.0.0.1:$PORT; harnesses $HARNESSES; evidence in $EVIDENCE"
cd "$WORKSPACE"
CUA_AGENTS_E2E_URL="http://127.0.0.1:$PORT" CUA_AGENTS_E2E_TOKEN="$TOKEN" \
    CUA_AGENTS_E2E_ENDPOINT="$ENDPOINT" CUA_AGENTS_E2E_KEY="$MOCK_KEY" \
    CUA_AGENTS_E2E_HARNESSES="$HARNESSES" CUA_AGENTS_E2E_EVIDENCE="$EVIDENCE" \
    timeout 3600 cargo test -q -p cua-agents --test e2e_live -- --nocapture --test-threads=1 2>&1 \
    | tee "$EVIDENCE/cargo-test.log"
