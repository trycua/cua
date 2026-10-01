#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Live run of the Spaces app's command layer, WITHOUT the GUI.
#
# Starts a linux container (gVisor by default, --memory=4g) with
# the spacesd published on a loopback port, starts `cua daemon` under a
# temp CUA_HOME, then runs `cargo test --test live_docker`, which drives
# AppCore (what every Tauri command calls): add Space by address, screenshot,
# send file (sha256 checked by the guest), open a desktop stream ticket and
# attach like the webview, the daemon media bridge, teleport a generated
# Firefox profile, delete. The daemon and the container are always removed.
#
#   run-live-docker.sh [--image REF] [--runtime runsc|runc] [--cua PATH] [--keep]
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TAURI="$(cd "$HERE/../.." && pwd)"
REPO="$(cd "$TAURI/../../.." && pwd)"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
IMAGE="cua-e2e-local/linux:docker-local-$(host_arch)"
RUNTIME=runsc
CUA="${CUA_BIN:-}"
KEEP=0
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --runtime) RUNTIME="$2"; shift 2 ;;
        --cua) CUA="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
if [ -z "$CUA" ]; then
    echo "==> building the cua CLI"
    (cd "$REPO/libs/cua" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" cargo build -q -p cua-cli)
    CUA="$REPO/libs/cua/target/debug/cua"
fi
[ -x "$CUA" ] || { echo "no cua binary at $CUA" >&2; exit 2; }

NAME="cua-e2e-spacesapp-$$"
TOKEN="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
HOME_DIR="$(mktemp -d)"
ENV_FILE="$(mktemp)"
chmod 600 "$ENV_FILE"
printf 'CUA_ENV_TOKEN=%s\n' "$TOKEN" >"$ENV_FILE"

cleanup() {
    rm -f "$ENV_FILE"
    CUA_HOME="$HOME_DIR" "$CUA" daemon stop >/dev/null 2>&1 || true
    if [ "$KEEP" = 1 ]; then
        echo "kept container $NAME and CUA_HOME $HOME_DIR"
    else
        docker rm -f "$NAME" >/dev/null 2>&1 || true
        rm -rf "$HOME_DIR"
    fi
}
trap cleanup EXIT

echo "==> $IMAGE ($RUNTIME) as $NAME"
docker run -d --name "$NAME" --runtime="$RUNTIME" --memory=4g --memory-swap=4g --shm-size=512m \
    --env-file "$ENV_FILE" -p 127.0.0.1::3211 "$IMAGE" >/dev/null
status=""
for _ in $(seq 1 120); do
    status="$(docker inspect -f '{{.State.Health.Status}}' "$NAME" 2>/dev/null || echo gone)"
    { [ "$status" = healthy ] || [ "$status" = gone ]; } && break
    sleep 1
done
[ "$status" = healthy ] || { echo "container health: $status" >&2; docker logs "$NAME" 2>&1 | tail -30; exit 1; }
PORT="$(docker port "$NAME" 3211/tcp | head -1 | sed 's/.*://')"
for _ in $(seq 1 60); do
    [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/health")" = 204 ] && break
    sleep 1
done
echo "==> spacesd on 127.0.0.1:$PORT"

echo "==> cua daemon (CUA_HOME=$HOME_DIR)"
CUA_HOME="$HOME_DIR" "$CUA" daemon start

cd "$TAURI"
CUA_SPACES_APP_E2E_URL="http://127.0.0.1:$PORT" CUA_SPACES_APP_E2E_TOKEN="$TOKEN" \
    CUA_SPACES_APP_E2E_HOME="$HOME_DIR" CUA_BIN="$CUA" CUA_HOME="$HOME_DIR" \
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
    timeout 900 cargo test --test live_docker -- --test-threads=1 --nocapture
