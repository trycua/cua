#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Run the core conformance suite (cua-spacesd-server) and the desktop suite
# (tests/linux_desktop.rs) against the REAL cua-spacesd inside the
# linux image, under runc or runsc (gVisor).
#
#   run-against-image.sh [runc|runsc] [image]
#
# The sandbox runs with --memory=4g; the test client runs in the
# cua-envdesktop-test image on a private docker network (nothing published).
# Fixtures (cua-spacesd-x11-pad, the GTK button) are copied into the sandbox and
# started through the driver's own ProcessService.
set -euo pipefail
RUNTIME="${1:-runc}"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
IMAGE="${2:-cua-e2e-local/linux:docker-local-$(host_arch)}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../../../../.." && pwd)"
NAME="cua-e2e-envdesktop-$RUNTIME-$$"
NET="cua-e2e-envdesktop-net-$$"
TOKEN="e2e-$(date +%s)-$RANDOM$RANDOM"
TARGET_VOL="${CUA_TEST_TARGET_VOLUME:-envdesktop-target}"
cleanup() {
    docker logs "$NAME" >"/tmp/$NAME.log" 2>&1 || true
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    docker network rm "$NET" >/dev/null 2>&1 || true
}
trap cleanup EXIT
docker network create "$NET" >/dev/null
docker run -d --name "$NAME" --network "$NET" --runtime="$RUNTIME" --shm-size=512m \
    --memory=4g --memory-swap=4g -e CUA_ENV_TOKEN="$TOKEN" "$IMAGE" >/dev/null
echo "==> $NAME ($RUNTIME): waiting for healthy"
for _ in $(seq 1 120); do
    status="$(docker inspect -f '{{.State.Health.Status}}' "$NAME" 2>/dev/null || echo gone)"
    [ "$status" = healthy ] && break
    [ "$status" = gone ] && { echo "sandbox exited"; exit 1; }
    sleep 1
done
echo "    health: $status"
# Fixtures into the sandbox.
docker run --rm -v "$TARGET_VOL:/target" -v "$REPO:/repo" -w /repo/libs/cua-spacesd \
    -e CARGO_TARGET_DIR=/target -e CARGO_PROFILE_DEV_DEBUG=0 -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=/repo/scripts/ci/linux/cc-rust-lld.sh cua-envdesktop-test \
    cargo build -q -p cua-spacesd-test-apps --bin cua-spacesd-x11-pad
docker run --rm -v "$TARGET_VOL:/target" alpine cat /target/debug/cua-spacesd-x11-pad >"/tmp/$NAME-x11-pad"
# Copy through `docker exec` (gVisor keeps /tmp inside the sandbox, where
# `docker cp` cannot write).
docker exec -i "$NAME" sh -c 'cat > /tmp/cua-spacesd-x11-pad' <"/tmp/$NAME-x11-pad"
docker exec -i "$NAME" sh -c 'cat > /tmp/gtk_button.py' \
    <"$REPO/libs/cua-spacesd/crates/cua-spacesd-test-apps/fixtures/linux/gtk_button.py"
docker exec "$NAME" chmod 0755 /tmp/cua-spacesd-x11-pad /tmp/gtk_button.py
rm -f "/tmp/$NAME-x11-pad"

run_suite() {
    docker run --rm --network "$NET" --memory=4g --memory-swap=4g \
        -v "$REPO:/repo" -v "$TARGET_VOL:/target" -v envdesktop-cargo:/usr/local/cargo/registry \
        -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=2 -e CARGO_PROFILE_DEV_DEBUG=0 -e CARGO_PROFILE_TEST_DEBUG=0 \
        -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=/repo/scripts/ci/linux/cc-rust-lld.sh \
        -e CUA_ENV_TEST_TARGET="http://$NAME:3211" -e CUA_ENV_TEST_TOKEN="$TOKEN" \
        -e CUA_ENV_LINUX_DESKTOP_TESTS=1 -e CUA_ENV_X11_PAD=/tmp/cua-spacesd-x11-pad \
        -e CUA_ENV_GTK_FIXTURE=/tmp/gtk_button.py \
        -w /repo/libs/cua-spacesd cua-envdesktop-test sh -c "timeout 1800 $1"
}
echo "==> core conformance suite against $RUNTIME"
[ -n "${SKIP_CORE:-}" ] || run_suite "cargo test -p cua-spacesd-server --test conformance -- --test-threads=4" 2>&1 | grep -E "^test |test result|panicked" || true
echo "==> desktop suite against $RUNTIME"
run_suite "cargo test -p cua-spacesd-desktop --test linux_desktop -- --test-threads=1 --nocapture ${DESKTOP_FILTER:-}" 2>&1 | grep -E "^test |test result|panicked|skews|assert|flashes" -A2 || true
