#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Builds linux/<host arch> cua-spacesd, cua-relay and the fake OIDC issuer
# (cua-relay example) in the linuxtest image and copies them to
# /target/e2e in the named target volume ($CUA_ENV_TARGET_VOLUME).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../.." && pwd)"
image="${CUA_ENV_TEST_IMAGE:-cua-spacesd-linuxtest}"
target_volume="${CUA_ENV_TARGET_VOLUME:-cua-e2e-host-target}"
cargo_volume="${CUA_ENV_CARGO_VOLUME:-cua-envcore-cargo}"
docker image inspect "$image" >/dev/null 2>&1 || docker build -t "$image" -f "$here/linux-test.Dockerfile" "$here"
# Big debug test binaries OOM GNU ld at 4 GiB: line-tables-only debug info,
# and rust-lld on aarch64 (scripts/ci/linux/cc-rust-lld.sh; x86_64 already
# links with rust-lld).
docker run --rm --memory=6g --memory-swap=6g -v "$repo:/repo" -v "$target_volume:/target" \
  -v "$cargo_volume:/usr/local/cargo/registry" -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=4 -e CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}" -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=/repo/scripts/ci/linux/cc-rust-lld.sh \
  -w /repo/libs/cua-spacesd "$image" bash -c '
    set -euo pipefail
    command -v nasm >/dev/null || (apt-get update -qq && apt-get install -y -qq nasm >/dev/null)
    cargo build --locked -p cua-spacesd -p cua-relay
    cargo build --locked -p cua-relay --example fake_oidc
    mkdir -p /target/e2e
    cp /target/debug/cua-spacesd /target/debug/cua-relay /target/debug/examples/fake_oidc /target/e2e/
    ls -la /target/e2e
  '
