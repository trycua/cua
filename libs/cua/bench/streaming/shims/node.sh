#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Bench lane "typescript-node": runs examples/streaming/typescript-node in
# bench mode (SCENARIO.md "Benchmark JSONL"). The harness sets CUA_ENV_URL,
# CUA_ENV_TOKEN, CUA_BENCH_JSONL, CUA_BENCH_TARGET, CUA_BENCH_SECONDS and
# CUA_BENCH_AUDIO. CUA_NODE_MEDIA=encoded switches to raw access units
# (bytes/keyframes per frame, no tc_ms).
#
# Exit codes: the example's (0 ok, 1 no frames), 2 missing env, 3 skipped
# (a prerequisite is not built/installed; the message says which).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../../.." && pwd)"
EX="$ROOT/examples/streaming/typescript-node"
SDK="$ROOT/libs/cua/typescript"
skip() { echo "skipped: $*" >&2; exit 3; }

for v in CUA_ENV_URL CUA_ENV_TOKEN CUA_BENCH_JSONL; do
    [ -n "${!v:-}" ] || { echo "error: $v is required (set by the bench harness)" >&2; exit 2; }
done

command -v node >/dev/null || skip "node not found (need Node >= 23.6 for type stripping)"
node -e 'const [a,b]=process.versions.node.split(".").map(Number);process.exit(a>23||(a===23&&b>=6)?0:1)' \
    || skip "node $(node --version) is too old (need >= 23.6)"
[ -f "$SDK/dist/index.js" ] || skip "SDK not built: (cd libs/cua/typescript && npm ci && npm run build)"
ls "$SDK"/node_modules/@trycua/cua-*/libcua_sdk.* >/dev/null 2>&1 \
    || skip "native cua-sdk not staged: (cd libs/cua && cargo build --release -p cua-sdk && node scripts/stage-uniffi-library.mjs --only=node)"
[ -e "$EX/node_modules/@trycua/cua" ] || skip "example deps missing: (cd examples/streaming/typescript-node && npm install)"

cd "$EX"
exec node main.ts
