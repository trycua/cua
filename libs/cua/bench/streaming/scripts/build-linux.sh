#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Build the benchmark harness for Linux (host arch) inside Docker, for the
# sidecar lanes (`run --sidecar`): output target/linux-<arch>/cua-bench-streaming.
#
# The repo is mounted read-only; cargo's registry and target dir live in
# named volumes (cua-e2e-bench-*). Memory is capped at 6 GiB.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH="$(cd "$HERE/.." && pwd)"
REPO_ROOT="$(cd "$BENCH/../../../.." && pwd)"
RUST_IMAGE="${RUST_IMAGE:-rust:1-bookworm}"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
arch="$(host_arch)"
out="$BENCH/target/linux-$arch"
mkdir -p "$out"
docker run --rm --memory=6g --memory-swap=6g \
    -v "$REPO_ROOT:/src:ro" \
    -v "cua-e2e-bench-cargo-registry-$arch:/usr/local/cargo/registry" \
    -v "cua-e2e-bench-target-$arch:/target" \
    -v "$out:/out" \
    -e CARGO_TARGET_DIR=/target -e CARGO_TERM_COLOR=never -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
    "$RUST_IMAGE" bash -euo pipefail -c '
        apt-get update -qq && apt-get install -y -qq --no-install-recommends clang cmake nasm protobuf-compiler libprotobuf-dev pkg-config >/dev/null
        cd /src/libs/cua/bench/streaming
        cargo build --release --locked
        install -m 0755 /target/release/cua-bench-streaming /out/cua-bench-streaming
    '
echo "built $out/cua-bench-streaming"
