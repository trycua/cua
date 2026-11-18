#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Bench lane launcher for the Rust streaming example (the cua-sdk crate's
# decoded media API, as opposed to the harness's own native `rust` lane).
#
# The harness sets CUA_ENV_URL, CUA_ENV_TOKEN, CUA_BENCH_JSONL,
# CUA_BENCH_TARGET, CUA_BENCH_SECONDS and CUA_BENCH_AUDIO.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EXAMPLE="$(cd "$HERE/../../../../../examples/streaming/rust" && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$EXAMPLE/target}"
BIN="$TARGET_DIR/release/cua-streaming-example"
if [ ! -x "$BIN" ]; then
    echo "skipped: rust-sdk lane: $BIN is not built (cd $EXAMPLE && cargo build --release)" >&2
    exit 3
fi
: "${CUA_BENCH_JSONL:?CUA_BENCH_JSONL must be set}"
: "${CUA_ENV_TOKEN:?CUA_ENV_TOKEN must be set}"
exec "$BIN"
