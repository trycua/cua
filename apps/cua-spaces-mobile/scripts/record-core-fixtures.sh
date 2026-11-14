#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Brings core/fixtures up to date with what the tests ask of the app core: records the calls the tests make, adds them
# to core/fixtures/calls.json, and has the real core answer them (`UPDATE_FIXTURES=1 cargo test --test fixtures`).
# A test that calls the core with something it has not been told before (a state that moves) needs a few rounds, so
# this repeats until nothing new is recorded. Review `git diff core/fixtures/results.json`: a changed answer is a changed
# sentence on the phone.
set -euo pipefail
# macOS shells default to 256 open files, too few for the cua-spaces test binaries (see scripts/ci/tests/test_nofile_limit.py).
[ "$(ulimit -Sn)" = unlimited ] || [ "$(ulimit -Sn)" -ge 10240 ] || ulimit -Sn 10240 2>/dev/null || ulimit -Sn "$(ulimit -Hn)" 2>/dev/null || true # nofile-limit
cd "$(dirname "$0")/.."
export CARGO_INCREMENTAL=0
for round in $(seq 1 30); do
  rm -f core/fixtures/pending.ndjson
  (cd core && UPDATE_FIXTURES=1 cargo test --test fixtures >/dev/null)
  RECORD_CORE_CALLS=1 pnpm exec jest --maxWorkers=2 >/dev/null 2>&1 || true
  out="$(node scripts/merge-core-calls.mjs)"
  echo "round $round: $out"
  case "$out" in "nothing pending"|"added 0 calls") break ;; esac
done
(cd core && UPDATE_FIXTURES=1 cargo test --test fixtures >/dev/null)
