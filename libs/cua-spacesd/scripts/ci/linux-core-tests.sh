#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Linux lane for the spacesd server core: unit tests plus the full
# conformance and in-process relay suites (1 GiB transfers, 60 s stream),
# the teleport receiver (cua-spacesd-teleport) and the SDK-sender -> driver
# teleport end-to-end test (tests/teleport-e2e), inside a memory-capped
# container. Teleport tests only ever act on fake hosts and temp homes.
#
# Tests run under cargo-nextest with the CI profile (.github/flaky/nextest.toml: one
# retry, JUnit output). The lane used to run plain `cargo test`, the only Linux lane
# with no retry, which is why a timing test (reconnect_storm, churn_bounded,
# share_links, ...) failed it about one run in three (CUA-3558). Each nextest run
# copies its JUnit file to $CUA_ENV_JUNIT_DIR so flaky.py can report what only passed
# on the retry.
#
# Shards, run as parallel CI jobs (the tunnel shard is relay-e2e.sh):
#   core      default features: server, relay, socks, teleport receiver
#   hosted    the hosted relay build (cua-relay/hosted, cua-spacesd-server/relay-hosted)
#   teleport  the cua-spacesd binary tests and tests/teleport-e2e
#   all       every shard in order (default; what a developer runs)
#
#   scripts/ci/linux-core-tests.sh [core|hosted|teleport|all]
#   CUA_ENV_TEST_BIG_BYTES=... CUA_ENV_TEST_LONG_SECS=... scripts/ci/...
set -euo pipefail
# macOS shells default to 256 open files, too few for the cua-spaces test binaries (see scripts/ci/tests/test_nofile_limit.py).
[ "$(ulimit -Sn)" = unlimited ] || [ "$(ulimit -Sn)" -ge 10240 ] || ulimit -Sn 10240 2>/dev/null || ulimit -Sn "$(ulimit -Hn)" 2>/dev/null || true # nofile-limit
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../.." && pwd)"
image="${CUA_ENV_TEST_IMAGE:-cua-spacesd-linuxtest}"
shard="${1:-all}"
case "$shard" in core|hosted|teleport|all) ;; *) echo "usage: $0 [core|hosted|teleport|all]" >&2; exit 2 ;; esac
junit_dir="${CUA_ENV_JUNIT_DIR:-${RUNNER_TEMP:-/tmp}/cua-core-junit}"
mkdir -p "$junit_dir"
if ! docker image inspect "$image" >/dev/null 2>&1; then
  docker build -t "$image" -f "$here/linux-test.Dockerfile" "$here"
fi
# Big debug test binaries OOM GNU ld at 4 GiB: line-tables-only debug info,
# and rust-lld on aarch64 (scripts/ci/linux/cc-rust-lld.sh; x86_64 already
# links with rust-lld).
exec docker run --rm --memory=4g --memory-swap=4g --tmpfs /tiny:size=1m \
  -v "$repo:/repo" \
  -v "$junit_dir:/junit" \
  -v "${CUA_ENV_TARGET_VOLUME:-cua-envcore-target}:/target" \
  -v "${CUA_ENV_CARGO_VOLUME:-cua-envcore-cargo}:/usr/local/cargo/registry" \
  -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=4 -e CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}" -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=/repo/scripts/ci/linux/cc-rust-lld.sh \
  -e CUA_ENV_TEST_SANDBOX=1 -e CUA_ENV_TEST_TINY_FS=/tiny \
  -e CUA_ENV_TEST_BIG_BYTES="${CUA_ENV_TEST_BIG_BYTES:-1073741824}" \
  -e CUA_ENV_TEST_LONG_SECS="${CUA_ENV_TEST_LONG_SECS:-60}" \
  -e SHARD="$shard" \
  -w /repo/libs/cua-spacesd "$image" bash -c '
    set -euo pipefail
    # nextest: one retry, JUnit copied out after every run (also when it fails).
    nx() {
      local name=$1 rc=0; shift
      timeout 1800 cargo nextest run --locked --config-file /repo/.github/flaky/nextest.toml --profile ci -j 4 "$@" || rc=$?
      cp /target/nextest/ci/junit.xml "/junit/$name.xml" 2>/dev/null || true
      return $rc
    }
    want() { [ "$SHARD" = all ] || [ "$SHARD" = "$1" ]; }
    # Doctests are not run by nextest; the ubuntu `test` job runs them for the workspace.
    if want core; then
      nx core -p cua-spacesd-server -p cua-relay -p cua-spacesd-socks -p cua-spacesd-teleport
    fi
    if want hosted; then
      # The hosted relay'"'"'s build of the relay (the `hosted` feature).
      nx hosted -p cua-spacesd-server -p cua-relay --features cua-relay/hosted,cua-spacesd-server/relay-hosted
    fi
    if want teleport; then
      nx bin -p cua-spacesd --bin cua-spacesd
      (cd tests/teleport-e2e && nx teleport-e2e)
    fi
  '
