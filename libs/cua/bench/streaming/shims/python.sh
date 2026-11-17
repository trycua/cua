#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Bench lane launcher for the Python streaming example.
#
# The harness sets CUA_ENV_URL, CUA_ENV_TOKEN, CUA_BENCH_JSONL,
# CUA_BENCH_TARGET, CUA_BENCH_SECONDS and CUA_BENCH_AUDIO; this runs
# examples/streaming/python/stream_example.py in bench mode from its venv.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EXAMPLE="$(cd "$HERE/../../../../../examples/streaming/python" && pwd)"
PY="$EXAMPLE/.venv/bin/python"

if [ ! -x "$PY" ] || ! "$PY" -c 'import cua' >/dev/null 2>&1; then
    echo "python lane: the cua binding is not built in $EXAMPLE/.venv" >&2
    echo "build it with: $EXAMPLE/setup.sh   (or --no-build to reuse target/release)" >&2
    exit 3
fi
: "${CUA_BENCH_JSONL:?CUA_BENCH_JSONL must be set}"
: "${CUA_ENV_TOKEN:?CUA_ENV_TOKEN must be set}"
export CUA_HEADLESS=1
exec "$PY" "$EXAMPLE/stream_example.py"
