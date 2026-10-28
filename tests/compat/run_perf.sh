#!/usr/bin/env bash
# The compat overhead guard on real Spaces (tests/compat/e2b/perf/overhead.py):
# the E2B SDK through the gateway against the same operations done directly
# through the Cua SDK. Needs a running Cua daemon.
#
#   tests/compat/run_perf.sh [--write] [-- cua compat serve options]
#
# Without --write it checks against tests/compat/e2b/perf/baseline.json and
# fails when a ratio is over 1.5x its baseline. $CUA, $BENCH_DIRECT override
# the binaries; $PERF_ROUNDS (default 3), $PERF_N, $PERF_MIB tune the run.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$ROOT/../.." && pwd)"
CUA="${CUA:-$REPO/libs/cua/target/debug/cua}"
BENCH="${BENCH_DIRECT:-$REPO/libs/cua/target/debug/examples/bench_direct}"
MODE=(--check "$ROOT/e2b/perf/baseline.json")
[ "${1:-}" = "--write" ] && { MODE=(--write "$ROOT/e2b/perf/baseline.json"); shift; }
[ "${1:-}" = "--" ] && shift
SERVE=("$@"); [ ${#SERVE[@]} -eq 0 ] && SERVE=(--on local --kind vm --cpus 2 --memory-mb 2048)
WORK="${PERF_WORK:-$(mktemp -d "${TMPDIR:-/tmp}/cua-e2b-perf.XXXXXX")}"; mkdir -p "$WORK"
EXTRA=()
[ -n "${CUA_COMPAT_LISTEN:-}" ] && EXTRA+=(--listen "$CUA_COMPAT_LISTEN")
[ -n "${CUA_COMPAT_HOST_PORT:-}" ] && EXTRA+=(--host-port "$CUA_COMPAT_HOST_PORT")
"$CUA" compat serve --provider e2b ${EXTRA[@]+"${EXTRA[@]}"} "${SERVE[@]}" >"$WORK/gateway.out" 2>"$WORK/gateway.log" &
GW=$!
trap 'kill $GW 2>/dev/null || true; wait $GW 2>/dev/null || true' EXIT
for _ in $(seq 3000); do grep -q "Clients export" "$WORK/gateway.out" 2>/dev/null && break; sleep 0.2; done
grep -q "Clients export" "$WORK/gateway.out" || { cat "$WORK/gateway.log"; exit 1; }
"$CUA" compat env --provider e2b ${EXTRA[@]+"${EXTRA[@]}"} --json >"$WORK/env.json"
# The direct side takes the same placement.
DIRECT="$BENCH"
for i in "${!SERVE[@]}"; do case "${SERVE[$i]}" in --on|--kind|--image|--cpus|--memory-mb) DIRECT="$DIRECT ${SERVE[$i]} ${SERVE[$((i+1))]}";; esac; done
PY="${PERF_PYTHON:-python3}"
exec "$PY" "$ROOT/e2b/perf/overhead.py" --direct "$DIRECT" --env "$WORK/env.json" \
  --n "${PERF_N:-100}" --mib "${PERF_MIB:-64}" --rounds "${PERF_ROUNDS:-3}" "${MODE[@]}"
