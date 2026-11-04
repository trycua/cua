#!/usr/bin/env bash
# Sharing a real Space with a second account, in Docker:
#
#   one network namespace (a holder container publishing every port):
#     fake OIDC issuer (127.0.0.1:$ISSUER_PORT)
#     cua-relay in account mode (127.0.0.1:$RELAY_PORT)
#     a Linux Space running the freshly built cua-spacesd (serve mode, 3211)
#   this machine: `cargo test -p cua-spaces --test share_space_e2e` with
#   three clients (ada, ada's second device, bob), each in a temp home.
#
# Sharing the namespace lets the host and the Space name the relay by the
# same URL (127.0.0.1:$RELAY_PORT), which is what a public relay gives in
# production. The Space's driver joins the relay itself
# (`SystemService.AttachRelay`); nothing inbound is opened for the relay.
#
#   run-share-space-e2e.sh [--image REF] [--keep] [--skip-build]
#
# Needs the Linux binaries from
# libs/cua-spacesd/scripts/ci/build-linux-e2e-bins.sh (run unless
# --skip-build). Every container gets a memory limit; all are removed on
# exit. Evidence (logs, transcript, the Space's access log) goes to
# $EVIDENCE (default: a temp dir).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "$HERE/../../../.." && pwd)" # libs/cua
REPO="$(cd "$WORKSPACE/../.." && pwd)"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
IMAGE="cua-e2e-local/linux:docker-local-$(host_arch)"
KEEP=0
BUILD=1
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --skip-build) BUILD=0; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
TARGET_VOLUME="${CUA_ENV_TARGET_VOLUME:-cua-e2e-host-target}"
BIN_IMAGE="${CUA_ENV_TEST_IMAGE:-cua-spacesd-linuxtest}"
RUN="cua-e2e-share-$$"
EVIDENCE="${EVIDENCE:-$(mktemp -d)}"
mkdir -p "$EVIDENCE"
free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
RELAY_PORT="$(free_port)"
ISSUER_PORT="$(free_port)"
SPACE_PORT="$(free_port)"
TOKEN="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"

cleanup() {
    status=$?
    for c in issuer relay space; do
        docker logs "$RUN-$c" >"$EVIDENCE/$c.log" 2>&1 || true
    done
    docker exec "$RUN-space" sh -c 'tail -n 400 /var/log/supervisor/cua-spacesd.log' >"$EVIDENCE/spacesd.log" 2>&1 || true
    if [ "$status" != 0 ]; then
        echo "---- evidence in $EVIDENCE"
        tail -n 30 "$EVIDENCE/spacesd.log" "$EVIDENCE/relay.log" 2>/dev/null || true
    fi
    if [ "$KEEP" != 1 ]; then
        docker rm -f "$RUN-space" "$RUN-relay" "$RUN-issuer" "$RUN-net" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

if [ "$BUILD" = 1 ]; then
    CUA_ENV_TARGET_VOLUME="$TARGET_VOLUME" "$REPO/libs/cua-spacesd/scripts/ci/build-linux-e2e-bins.sh"
fi

echo "==> network namespace (relay $RELAY_PORT, issuer $ISSUER_PORT, space $SPACE_PORT)"
docker run -d --name "$RUN-net" --memory=64m \
    -p "127.0.0.1:$RELAY_PORT:$RELAY_PORT" -p "127.0.0.1:$ISSUER_PORT:$ISSUER_PORT" \
    -p "127.0.0.1:$SPACE_PORT:3211" "$BIN_IMAGE" sleep infinity >/dev/null
NET=(--network "container:$RUN-net")

echo "==> fake OIDC issuer"
docker run -d --name "$RUN-issuer" --memory=512m "${NET[@]}" -v "$TARGET_VOLUME:/target:ro" "$BIN_IMAGE" \
    /target/e2e/fake_oidc --listen "0.0.0.0:$ISSUER_PORT" --issuer "http://127.0.0.1:$ISSUER_PORT" \
    --sub ada --email ada@example.com >/dev/null

echo "==> cua-relay (account mode)"
docker run -d --name "$RUN-relay" --memory=1g "${NET[@]}" -v "$TARGET_VOLUME:/target:ro" \
    -e RUST_LOG=info,cua_relay=debug "$BIN_IMAGE" \
    /target/e2e/cua-relay --listen "0.0.0.0:$RELAY_PORT" --oidc-issuer "http://127.0.0.1:$ISSUER_PORT" >/dev/null

RELAY_URL="http://127.0.0.1:$RELAY_PORT"
ISSUER_URL="http://127.0.0.1:$ISSUER_PORT"
for _ in $(seq 1 150); do
    curl -fsS -o /dev/null "$RELAY_URL/healthz" && curl -fsS -o /dev/null "$ISSUER_URL/healthz" && break
    sleep 0.2
done
curl -fsS -o /dev/null "$RELAY_URL/healthz"

echo "==> the Space: $IMAGE with the freshly built cua-spacesd"
ENV_FILE="$(mktemp)"
chmod 600 "$ENV_FILE"
printf 'CUA_ENV_TOKEN=%s\n' "$TOKEN" >"$ENV_FILE"
docker run -d --name "$RUN-space" --memory=4g --memory-swap=4g --shm-size=512m "${NET[@]}" \
    --env-file "$ENV_FILE" -e CUA_AUDIO=false -e CUA_NOVNC=false \
    -v "$TARGET_VOLUME:/target:ro" -e CUA_SPACESD_BIN=/target/e2e/cua-spacesd "$IMAGE" >/dev/null
rm -f "$ENV_FILE"
SPACE_URL="http://127.0.0.1:$SPACE_PORT"
for _ in $(seq 1 120); do
    [ "$(curl -s -o /dev/null -w '%{http_code}' "$SPACE_URL/health")" = 204 ] && break
    sleep 1
done
[ "$(curl -s -o /dev/null -w '%{http_code}' "$SPACE_URL/health")" = 204 ] || { echo "the Space never became healthy" >&2; exit 1; }
docker exec "$RUN-space" sh -c 'cua-spacesd --version 2>/dev/null || /target/e2e/cua-spacesd --version' | tee "$EVIDENCE/spacesd-version.txt" || true

echo "==> cargo test -p cua-spaces --test share_space_e2e"
cd "$WORKSPACE"
CUA_SHARE_E2E_RELAY="$RELAY_URL" CUA_SHARE_E2E_ISSUER="$ISSUER_URL" \
    CUA_SHARE_E2E_SPACE="$SPACE_URL" CUA_SHARE_E2E_TOKEN="$TOKEN" CUA_SHARE_E2E_EVIDENCE="$EVIDENCE" \
    CUA_ENV_TEST_SANDBOX=1 CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
    timeout 1200 cargo test -p cua-spaces --test share_space_e2e -- --nocapture --test-threads=1 2>&1 | tee "$EVIDENCE/cargo-test.log"
status="${PIPESTATUS[0]}"
[ "$status" = 0 ] && echo "==> share space e2e: PASS (evidence: $EVIDENCE)"
exit "$status"
