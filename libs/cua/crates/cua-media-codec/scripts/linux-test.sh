#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Runs the cua-media-codec Linux suite inside a container: OpenH264 video
# tests, Opus tests, a real PulseAudio null-sink capture/uplink test and
# (optionally) the encode benchmarks. Memory-capped.
#
#   libs/cua/crates/cua-media-codec/scripts/linux-test.sh [--bench]
set -euo pipefail
repo="$(cd "$(dirname "$0")/../../../../.." && pwd)"
bench="${1:-}"
# Big debug test binaries OOM GNU ld at 4 GiB: line-tables-only debug info,
# and rust-lld on aarch64 (scripts/ci/linux/cc-rust-lld.sh; x86_64 already
# links with rust-lld).
docker run --rm --memory=4g --memory-swap=4g --cpus=4 \
  -v "$repo":/src:ro \
  -v cua-codec-cargo:/usr/local/cargo/registry \
  -v cua-codec-target:/target \
  -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=4 -e CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}" -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=/src/scripts/ci/linux/cc-rust-lld.sh \
  -e BENCH="$bench" \
  rust:1-bookworm bash -euc '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y -qq pulseaudio pulseaudio-utils libopus-dev pkg-config protobuf-compiler >/dev/null
    # PulseAudio as root in a container: user mode, no idle exit.
    pulseaudio -D --exit-idle-time=-1 --disallow-exit 2>/dev/null || true
    for i in $(seq 1 50); do pactl info >/dev/null 2>&1 && break; sleep 0.1; done
    pactl info | grep -E "Server (Name|Version)"
    cd /src/libs/cua
    export CUA_CODEC_TEST_PULSE=1
    timeout 1500 cargo test -p cua-media-codec --features proto --no-fail-fast -- --test-threads=4 --nocapture 2>&1 | grep -vE "^\s+Compiling|OpenH264\] this" || true
    timeout 300 cargo run -q -p cua-media-codec --bin cua-codec-probe -- --isolated
    if [ "$BENCH" = "--bench" ]; then
      timeout 1500 cargo bench -p cua-media-codec --bench encode 2>&1 | grep -E "^(encode_h264|opus)|time:"
    fi
  '
