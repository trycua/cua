#!/usr/bin/env bash
# Fork fan-out benchmark: a bake VM dirties a few hundred MB of RAM, is
# checkpointed, and the checkpoint is forked in waves (every fork of a wave
# started at once). Runs the opt-in integration test
# `cua-daemon/tests/qemu_fork_fanout.rs` (release build), which checks that
# every fork holds the world's state, is independent of the others, and
# shares the world's unwritten pages, then writes its numbers as JSON.
#
#   DISK=<linux Spaces disk (qcow2/raw)> bash run.sh [out.json]
#
# Environment (defaults in brackets):
#   DISK        a Linux Spaces containerDisk with cua-spacesd (required)
#   WAVES       fork waves [8,32,64]
#   MEMORY_MB   RAM of the world and of every fork [2048]
#   DIRTY_MB    RAM the world dirties before its checkpoint [300]
#   FORKABLE    1: the world runs as a fork source (shared RAM file); 0:
#               anonymous RAM, the checkpoint loads its stream into a file [1]
#   WARM        1: golden snapshots on (a bake VM of an image whose cua-spacesd
#               can become a new machine starts warm) [1]
#   EXPECT_WARM 1: fail unless the bake VM started warm [0]
#   ROOT        QEMU state directory [a temp dir]; removed after the run
#
# Then: python3 check.py out.json baseline.json --host <class>
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="${1:-$PWD/fork-fanout.json}"
: "${DISK:?set DISK to a Linux Spaces disk}"
ROOT_DIR="${ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/cua-fork-fanout.XXXXXX")}"
trap 'pkill -f "$ROOT_DIR" 2>/dev/null || true; rm -rf "$ROOT_DIR"' EXIT

cd "$HERE/../.."
bin=$(cargo test --locked --release -p cua-daemon --test qemu_fork_fanout --no-run \
    --message-format=json 2>/dev/null |
    python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m.get("target", {}).get("name") == "qemu_fork_fanout" and m.get("executable"):
        print(m["executable"])')
[ -x "$bin" ] || { echo "could not build the fan-out test" >&2; exit 1; }

CUA_VMM_TEST_QEMU=1 CUA_VMM_TEST_DISK="$DISK" \
    CUA_FORK_WAVES="${WAVES:-8,32,64}" CUA_FORK_MEMORY_MB="${MEMORY_MB:-2048}" \
    CUA_FORK_DIRTY_MB="${DIRTY_MB:-300}" CUA_FORK_FORKABLE="${FORKABLE:-1}" \
    CUA_FORK_WARM="${WARM:-1}" CUA_FORK_EXPECT_WARM="${EXPECT_WARM:-0}" \
    CUA_FORK_ROOT="$ROOT_DIR" CUA_FORK_BENCH_OUT="$OUT" \
    RUST_LOG="${RUST_LOG:-cua_vmm=info}" \
    "$bin" --ignored --nocapture --test-threads=1
echo "results: $OUT"
