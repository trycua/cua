#!/usr/bin/env bash
# Host setup in Linux containers only (never on the developer's machine):
#  1. builds `cua` and `cua-fake-relay` for Linux in rust:1-bookworm,
#  2. process lane: a plain container, `--runner process`,
#  3. systemd lane: a container running systemd as PID 1, `--runner systemd`.
# Every container is named cua-e2e-host-* and removed afterwards; memory is
# capped at 4 GiB. Usage: run.sh [process|systemd|all] (default all).
set -euo pipefail
LANES="${1:-all}"
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../../../../.." && pwd)"
OUT="${CUA_HOST_TEST_OUT:-$HERE/.out}"
mkdir -p "$OUT"
cleanup() { docker rm -f cua-e2e-host-process cua-e2e-host-systemd >/dev/null 2>&1 || true; }
trap cleanup EXIT

echo "== build (linux $(docker info --format '{{.Architecture}}'))"
docker volume create cua-e2e-host-target >/dev/null
docker volume create cua-e2e-host-cargo >/dev/null
docker volume create cua-e2e-host-rustup >/dev/null
# Big debug test binaries OOM GNU ld at 4 GiB: line-tables-only debug info,
# and rust-lld on aarch64 (scripts/ci/linux/cc-rust-lld.sh; x86_64 already
# links with rust-lld).
docker run --rm --name cua-e2e-host-build --memory=4g --memory-swap=4g \
  -v "$REPO:/src:ro" -v cua-e2e-host-target:/target -v cua-e2e-host-cargo:/usr/local/cargo/registry -v cua-e2e-host-rustup:/usr/local/rustup \
  -v "$OUT:/out" -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=4 -e CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}" -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=/src/scripts/ci/linux/cc-rust-lld.sh \
  -w /src/libs/cua \
  rust:1-bookworm bash -c '
    set -e
    command -v protoc >/dev/null || (apt-get update -qq && apt-get install -y -qq protobuf-compiler >/dev/null)
    cargo build -q -p cua-cli --bin cua
    cargo build -q -p cua-host --features testing --bin cua-fake-relay
    cp /target/debug/cua /target/debug/cua-fake-relay /out/'
cp "$HERE/stub-driver" "$HERE/host-flow.sh" "$OUT/"
chmod +x "$OUT/stub-driver" "$OUT/host-flow.sh"

if [ "$LANES" = all ] || [ "$LANES" = process ]; then
  echo "== process lane"
  docker run --rm --init --name cua-e2e-host-process --memory=4g --memory-swap=4g \
    -v "$OUT:/opt/cua:ro" -e RUNNER=process debian:bookworm-slim \
    bash -c 'apt-get update -qq >/dev/null && apt-get install -y -qq procps >/dev/null && /opt/cua/host-flow.sh'
fi

if [ "$LANES" = all ] || [ "$LANES" = systemd ]; then
  echo "== systemd lane"
  docker build -q -t cua-e2e-host-systemd:local -f "$HERE/Dockerfile.systemd" "$HERE" >/dev/null
  docker run -d --name cua-e2e-host-systemd --memory=4g --memory-swap=4g \
    --privileged --cgroupns=host -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
    --tmpfs /run --tmpfs /run/lock -v "$OUT:/opt/cua:ro" cua-e2e-host-systemd:local >/dev/null
  for _ in $(seq 1 60); do
    state="$(docker exec cua-e2e-host-systemd systemctl is-system-running 2>/dev/null || true)"
    case "$state" in running|degraded) break ;; esac
    sleep 1
  done
  echo "systemd: $state"
  docker exec -e RUNNER=systemd cua-e2e-host-systemd /opt/cua/host-flow.sh
fi
echo "== host setup docker test passed ($LANES)"
