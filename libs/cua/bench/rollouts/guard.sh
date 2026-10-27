#!/usr/bin/env bash
# Perf guard for RL-rollout Spaces (headless gVisor containers): cold start,
# warm-pool claims, idle cost and a rollout-shaped load, through the SDK
# (`rollout_bench`) and a daemon of its own. Writes $OUT/guard.json and checks
# it with check.py against invariants and, for a host with a committed
# baseline (baseline/<BENCH_HOST>.json), a 1.5x threshold.
#
#   CUA=... ROLLOUT_BENCH=... BENCH_HOST=linux-arm64-ci bash guard.sh
#
# Env: CUA, ROLLOUT_BENCH (binaries), OUT (default ./guard-out), RUNTIME
# (default gvisor), IMAGE (default ghcr.io/trycua/linux:24.04-slim), N_ROLL
# (sandboxes in the rollout, default 32), BENCH_HOST (baseline key).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CUA=${CUA:-cua}; RB=${ROLLOUT_BENCH:-rollout_bench}
OUT=${OUT:-$PWD/guard-out}; RUNTIME=${RUNTIME:-gvisor}
IMAGE=${IMAGE:-ghcr.io/trycua/linux:24.04-slim}; N_ROLL=${N_ROLL:-32}
mkdir -p "$OUT"
export DO_NOT_TRACK=1 CUA_TELEMETRY=0 CUA_POOL_AUTO=off CUA_GVISOR_WARM=0
export CUA_HOME=${CUA_HOME:-$(mktemp -d)}
ulimit -n 65536 2>/dev/null || true
# Foreground in the background: on a Mac with the Cua Spaces app installed a
# bare `daemon start` hands off to the app's bundled daemon, not this build.
"$CUA" daemon start --foreground >"$OUT/daemon.log" 2>&1 &
for _ in $(seq 1 100); do "$CUA" daemon status >/dev/null 2>&1 && break; sleep 0.2; done
"$CUA" daemon status >/dev/null
trap '"$RB" delete --prefix g- --conc 16 >/dev/null 2>&1 || true; "$CUA" daemon stop >/dev/null 2>&1 || true' EXIT
K=(--image "$IMAGE" --kind container --runtime "$RUNTIME" --cpus 1 --memory-mb 1024)
r() { "$RB" "$@" 2>>"$OUT/stderr.log" | tee -a "$OUT/raw.jsonl" | grep '"summary":true' | tail -1; }
# The first create pulls the image (and on a Mac boots the runtime VM).
r create --prefix g-w- --n 1 "${K[@]}" --headless --label warmup >/dev/null
desktop=$(r create --prefix g-d- --n 3 "${K[@]}" --label desktop)
serial=$(r create --prefix g-s- --n 10 "${K[@]}" --headless --label headless-serial)
batch=$(r create --prefix g-b- --n 16 --conc 16 "${K[@]}" --headless --label headless-b16)
r create --prefix g-r- --n "$N_ROLL" --conc 16 "${K[@]}" --headless --keep --label keep >/dev/null
# Idle cost: CPU of a headless Space's cua-spacesd over 5 s (no display, so no
# desktop services polling).
idle=$("$CUA" sb exec g-r-0 'P=1; a=$(awk "{print \$14+\$15}" /proc/$P/stat); sleep 5; b=$(awk "{print \$14+\$15}" /proc/$P/stat); echo $(( (b-a) * 100 / (5 * $(getconf CLK_TCK)) ))')
rollout=$(r rollout --prefix g-r- --steps 20 --label rollout)
"$RB" delete --prefix g-r- --conc 16 >/dev/null 2>&1
pool=$(r pool --name gpool --n 16 --prefix g-p- "${K[@]}" --headless --label pool)
python3 - "$OUT/guard.json" <<PY
import json, sys
j = lambda s: json.loads(s) if s.strip() else {}
out = {
  "desktop_create_p50_ms": j('''$desktop''').get("p50"),
  "headless_create_p50_ms": j('''$serial''').get("p50"),
  "headless_b16_wall_ms": j('''$batch''').get("wall_ms"),
  "create_failures": sum(j(x).get("failed", 0) or 0 for x in ('''$desktop''', '''$serial''', '''$batch''')),
  "idle_spacesd_cpu_pct": int("$idle".strip() or -1),
  "rollout_steps_per_sec": j('''$rollout''').get("steps_per_sec"),
  "rollout_errors": j('''$rollout''').get("errors"),
  "pool_claim_batch_p50_ms": j('''$pool''').get("p50"),
  "rollout_sandboxes": $N_ROLL,
}
json.dump(out, open(sys.argv[1], "w"), indent=2)
print(json.dumps(out, indent=2))
PY
BASE="$HERE/baseline/${BENCH_HOST:-none}.json"
if [ -f "$BASE" ]; then
  python3 "$HERE/check.py" "$OUT/guard.json" --baseline "$BASE"
else
  echo "no baseline for host ${BENCH_HOST:-(unset)}: checking the invariants only"
  python3 "$HERE/check.py" "$OUT/guard.json"
fi
