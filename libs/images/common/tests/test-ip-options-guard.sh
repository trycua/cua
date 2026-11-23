#!/usr/bin/env bash
# Opt-in runtime test for the CVE-2026-98432 interim guard. Builds a throwaway
# systemd Ubuntu container carrying the guard exactly as common/vm/Dockerfile
# installs it, boots it (privileged, its own network namespace and cgroup), and
# runs ip-options-guard-guest.sh inside. Crafted packets only ever travel on the
# container's own loopback and a veth pair between two of its namespaces.
#
#   CUA_TEST_DOCKER=1 libs/images/common/tests/test-ip-options-guard.sh
#
# Uses whatever docker the environment points at (set DOCKER_HOST / DOCKER_CONFIG
# for an isolated engine). Stops and removes its container on exit.
set -euo pipefail
[ "${CUA_TEST_DOCKER:-}" = 1 ] || { echo "skipped: set CUA_TEST_DOCKER=1 to run (needs a Docker engine that allows --privileged)"; exit 0; }
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CTX="$(cd "$HERE/../.." && pwd)"   # libs/images
NAME="cua-ipopts-test-$$"
IMG="cua-ipopts-test:$$"
cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; docker rmi -f "$IMG" >/dev/null 2>&1 || true; }
trap cleanup EXIT
docker build -q -f "$HERE/ip-options-guard.Dockerfile" -t "$IMG" "$CTX" >/dev/null
docker run -d --name "$NAME" --privileged --cgroupns=host --tmpfs /run --tmpfs /run/lock \
    -v /sys/fs/cgroup:/sys/fs/cgroup:rw "$IMG" >/dev/null
for _ in $(seq 1 60); do
    st="$(docker exec "$NAME" systemctl is-system-running 2>/dev/null || true)"
    case "$st" in running|degraded) break ;; esac; sleep 1
done
docker exec "$NAME" /opt/cua/tests/ip-options-guard-guest.sh
