#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Linux lane for the spacesd server core: unit tests plus the full
# conformance and in-process relay suites (1 GiB transfers, 60 s stream),
# the teleport receiver (cua-spacesd-teleport) and the SDK-sender -> driver
# teleport end-to-end test (tests/teleport-e2e), inside a memory-capped
# container. Teleport tests only ever act on fake hosts and temp homes.
#
#   scripts/ci/linux-core-tests.sh            # build image if needed, run all
#   scripts/ci/linux-core-tests.sh core       # one part (CI runs them side by side):
#                                             #   core    server, relay, socks, teleport
#                                             #   hosted  the same with the relay's hosted features
#                                             #   daemon  the cua-spacesd binary and tests/teleport-e2e
#   CUA_ENV_TEST_BIG_BYTES=... CUA_ENV_TEST_LONG_SECS=... scripts/ci/...
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../.." && pwd)"
part="${1:-all}"
case "$part" in all|core|hosted|daemon) ;; *) echo "unknown part $part (all, core, hosted, daemon)" >&2; exit 2 ;; esac
# In CI the container compiles through the job's sccache.
# shellcheck source=/dev/null
. "$repo/scripts/ci/lib/docker-sccache.sh"
docker_sccache_args
image="${CUA_ENV_TEST_IMAGE:-cua-spacesd-linuxtest}"
if ! docker image inspect "$image" >/dev/null 2>&1; then
  docker build -t "$image" -f "$here/linux-test.Dockerfile" "$here"
fi
# Big debug test binaries OOM GNU ld at 4 GiB: line-tables-only debug info,
# and rust-lld on aarch64 (scripts/ci/linux/cc-rust-lld.sh; x86_64 already
# links with rust-lld).
exec docker run --rm --memory=4g --memory-swap=4g --tmpfs /tiny:size=1m \
  ${DOCKER_SCCACHE[@]+"${DOCKER_SCCACHE[@]}"} \
  -v "$repo:/repo" \
  -v "${CUA_ENV_TARGET_VOLUME:-cua-envcore-target}:/target" \
  -v "${CUA_ENV_CARGO_VOLUME:-cua-envcore-cargo}:/usr/local/cargo/registry" \
  -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=4 -e CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}" -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=/repo/scripts/ci/linux/cc-rust-lld.sh \
  -e CUA_ENV_TEST_SANDBOX=1 -e CUA_ENV_TEST_TINY_FS=/tiny \
  -e CUA_ENV_TEST_BIG_BYTES="${CUA_ENV_TEST_BIG_BYTES:-1073741824}" \
  -e CUA_ENV_TEST_LONG_SECS="${CUA_ENV_TEST_LONG_SECS:-60}" \
  -e PART="$part" \
  -w /repo/libs/cua-spacesd "$image" bash -c "$(declare -f docker_sccache_stop)"'
    set -euo pipefail
    trap docker_sccache_stop EXIT
    want() { [ "$PART" = all ] || [ "$PART" = "$1" ]; }
    if want core; then
      timeout 1800 cargo test --locked -p cua-spacesd-server -p cua-relay -p cua-spacesd-socks -p cua-spacesd-teleport -- --test-threads=4
    fi
    if want hosted; then
      # The hosted relay'"'"'s build of the relay (the `hosted` feature).
      timeout 1800 cargo test --locked -p cua-spacesd-server -p cua-relay --features cua-relay/hosted,cua-spacesd-server/relay-hosted -- --test-threads=4
    fi
    if want daemon; then
      timeout 900 cargo test --locked -p cua-spacesd --bin cua-spacesd -- --test-threads=4
      (cd tests/teleport-e2e && timeout 900 cargo test --locked -- --test-threads=4)
    fi
  '
