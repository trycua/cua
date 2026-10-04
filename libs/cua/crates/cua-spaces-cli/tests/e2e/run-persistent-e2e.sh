#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Persistent agents end to end (crates/cua-spaces-cli/tests/e2e_persistent.rs):
# a real `cua daemon` with a temporary HOME and CUA_HOME creates local
# Docker Spaces (runc) and runs Claude Code in them against the scripted
# mock provider (cua-mock-llm). This script starts the mock on the default
# bridge network (--memory=256m), passes its address, runs the test and
# always removes the mock and any Space container the test left behind.
#
#   run-persistent-e2e.sh [--image REF] [--evidence DIR]
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "$HERE/../../../.." && pwd)" # libs/cua
case "$(uname -m)" in arm64|aarch64) ARCH=aarch64; DARCH=arm64 ;; *) ARCH=x86_64; DARCH=amd64 ;; esac
IMAGE="cua-e2e-local/linux:docker-local-$DARCH"
EVIDENCE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
EVIDENCE="${EVIDENCE:-$(mktemp -d)}"
mkdir -p "$EVIDENCE"
MOCK="cua-e2e-persistent-mock-$$"
MOCK_KEY="mock-$(head -c 12 /dev/urandom | od -An -tx1 | tr -d ' \n')"
MOCK_ENV="$(mktemp)"; chmod 600 "$MOCK_ENV"
printf 'CUA_MOCK_LLM_KEY=%s\n' "$MOCK_KEY" >"$MOCK_ENV"

cleanup() {
    rm -f "$MOCK_ENV"
    docker logs "$MOCK" >"$EVIDENCE/mock-llm.log" 2>&1 || true
    docker rm -f "$MOCK" >/dev/null 2>&1 || true
    # Spaces the test created are named pe2e-*; remove any it left behind.
    for c in $(docker ps -aq --filter "name=pe2e-"); do docker rm -f "$c" >/dev/null 2>&1 || true; done
}
trap cleanup EXIT

echo "==> building cua-mock-llm ($ARCH-unknown-linux-musl) and cua"
(cd "$WORKSPACE" && cargo zigbuild --release -q -p cua-mock-llm --target "$ARCH-unknown-linux-musl")
BIN="${CARGO_TARGET_DIR:-$WORKSPACE/target}/$ARCH-unknown-linux-musl/release/cua-mock-llm"
[ -f "$BIN" ] || { echo "cua-mock-llm was not built at $BIN" >&2; exit 1; }
docker run -d --name "$MOCK" --memory=256m --env-file "$MOCK_ENV" \
    -v "$BIN:/usr/local/bin/cua-mock-llm:ro" debian:bookworm-slim \
    cua-mock-llm --listen 0.0.0.0:8787 >/dev/null
ENDPOINT="http://$(docker inspect -f '{{.NetworkSettings.Networks.bridge.IPAddress}}' "$MOCK"):8787"
echo "==> mock provider at $ENDPOINT; image $IMAGE; evidence in $EVIDENCE"
cd "$WORKSPACE"
CUA_PERSISTENT_E2E=1 CUA_PERSISTENT_E2E_ENDPOINT="$ENDPOINT" CUA_PERSISTENT_E2E_KEY="$MOCK_KEY" \
    CUA_PERSISTENT_E2E_IMAGE="$IMAGE" CUA_PERSISTENT_E2E_EVIDENCE="$EVIDENCE" \
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" RUST_MIN_STACK=16777216 \
    timeout 3600 cargo test -q -p cua-spaces-cli --test e2e_persistent -- --nocapture --test-threads=1 2>&1 \
    | tee "$EVIDENCE/cargo-test.log"
exit "${PIPESTATUS[0]}"
