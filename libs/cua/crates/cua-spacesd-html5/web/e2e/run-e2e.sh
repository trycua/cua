#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Browser e2e of the cua-spacesd HTML5 viewer against a throwaway local
# sandbox container. Opt-in (starts containers); cleans up what it creates.
#
#   run-e2e.sh --image REF [--browsers chromium,firefox,webkit] [--out DIR] [--runtime runc|runsc]
#
# The sandbox runs with a 4 GiB cap; Playwright runs in its own container
# (4 GiB cap) that shares the sandbox's network namespace, so the page is
# http://127.0.0.1:3211/viewer/ (a secure context). Nothing runs on the
# host's desktop.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEB="$(cd "$HERE/.." && pwd)"
IMAGE=""; BROWSERS="chromium,firefox,webkit"; OUT="$PWD/viewer-e2e"; RUNTIME=runc
PW_IMAGE="${PW_IMAGE:-mcr.microsoft.com/playwright:v1.63.0-noble}"
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --browsers) BROWSERS="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --runtime) RUNTIME="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$IMAGE" ] || { echo "--image is required" >&2; exit 2; }
mkdir -p "$OUT"
NAME="cua-e2e-webviewer-$$"
TOKEN="e2e-$(date +%s)-$RANDOM$RANDOM"
# Under $HOME: Colima and Docker Desktop share it with the VM, /var/folders not.
mkdir -p "$HOME/.cache"
WORK="$(mktemp -d "$HOME/.cache/cua-e2e-webviewer.XXXXXX")"
cleanup() {
    docker exec "$NAME" sh -c 'tail -n 200 /var/log/supervisor/cua-spacesd.log' >"$OUT/spacesd.log" 2>&1 || true
    docker rm -f "$NAME-pw" "$NAME" >/dev/null 2>&1 || true
    rm -rf "$WORK"
}
trap cleanup EXIT

"$WEB/node_modules/.bin/esbuild" "$HERE/viewer.e2e.ts" --bundle --platform=node --format=esm \
    --external:playwright --outfile="$WORK/e2e.mjs" --log-level=warning

docker run -d --name "$NAME" --runtime="$RUNTIME" --shm-size=512m --memory=4g --memory-swap=4g \
    -e CUA_ENV_TOKEN="$TOKEN" "$IMAGE" >/dev/null
for _ in $(seq 1 90); do
    [ "$(docker inspect -f '{{.State.Health.Status}}' "$NAME")" = healthy ] && break
    sleep 1
done
for f in grid form tone; do docker exec "$NAME" cua-fixtures start "$f" >/dev/null; done
# CUA_E2E_LOCAL_ASSETS=1 serves web/../assets from the test container instead
# of the page embedded in the image's cua-spacesd (frontend iteration).
ASSET_ARGS=()
if [ "${CUA_E2E_LOCAL_ASSETS:-0}" = 1 ]; then
    cp -R "$WEB/../assets" "$WORK/assets"
    ASSET_ARGS=(-e ASSETS_DIR=/work/assets)
fi
docker run --rm --name "$NAME-pw" --network "container:$NAME" --memory=4g --memory-swap=4g --shm-size=1g \
    -e CUA_ENV_TOKEN="$TOKEN" -e BROWSERS="$BROWSERS" -e OUT=/out ${ASSET_ARGS[@]+"${ASSET_ARGS[@]}"} \
    -v "$WORK:/work" -v "$OUT:/out" -w /work "$PW_IMAGE" \
    bash -c 'npm init -y >/dev/null && npm i --no-audit --no-fund --silent playwright@1.63.0 >/dev/null && timeout 1200 node e2e.mjs'
