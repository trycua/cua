#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Bench lane "typescript-web": runs the examples/streaming/typescript-web page
# in Playwright's headless Chromium (fresh temporary profile) in bench mode.
# The page decodes with WebCodecs, so frame lines carry tc_ms. The harness
# sets CUA_ENV_URL, CUA_ENV_TOKEN, CUA_BENCH_JSONL, CUA_BENCH_TARGET,
# CUA_BENCH_SECONDS and CUA_BENCH_AUDIO.
#
# Exit codes: the page's (0 ok, 1 no frames), 2 missing env, 3 "skipped: …"
# (Playwright, its Chromium, or the SDK's browser build is unavailable).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../../.." && pwd)"
EX="$ROOT/examples/streaming/typescript-web"
skip() { echo "skipped: $*" >&2; exit 3; }

for v in CUA_ENV_URL CUA_ENV_TOKEN CUA_BENCH_JSONL; do
    [ -n "${!v:-}" ] || { echo "error: $v is required (set by the bench harness)" >&2; exit 2; }
done

command -v node >/dev/null || skip "node not found"
[ -d "$EX/node_modules/playwright" ] \
    || skip "Playwright not installed: (cd examples/streaming/typescript-web && npm install && npm run install-browser)"
# Browsers are installed into node_modules (PLAYWRIGHT_BROWSERS_PATH=0),
# never a shared cache, and never the user's own Chrome.
export PLAYWRIGHT_BROWSERS_PATH=0
cd "$EX"
exec node headless.mjs
