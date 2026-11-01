#!/usr/bin/env bash
# macOS microVM (vz) checkpoint / restore / fork benchmark and perf guard,
# through the real `cua` CLI and daemon (the path a user or an agent takes).
#
# Usage (an Apple silicon Mac, macOS 14+, with cua-vz and the guest init
# next to `cua`, or CUA_VZ / CUA_MICROVM_INIT set for the daemon):
#   CUA=/path/to/cua OUT=results bash bench.sh            # measure
#   (with a cua daemon of that build running: on a Mac with the Cua Spaces
#   app installed, `CUA_SPACES_CLI=/nonexistent cua daemon start` keeps
#   `cua daemon start` from handing the daemon to the app's own cua)
#   python3 check.py results/summary.json baseline.json    # guard
#
# Environment:
#   CUA        the cua binary (default: cua on PATH)
#   IMAGE      Space image (default ghcr.io/trycua/linux:24.04-slim)
#   N          sequential samples per measurement (default 5)
#   FANOUT     forks of one checkpoint started at once (default 8)
#   CPUS, MEM  Space size (default 2 vCPU, 2GB)
#   SHM_MIB    MiB the source writes to its tmpfs before the checkpoint
#              (memory every fork restores; default 256)
#
# What it measures (wall time of each CLI command; `create` and `fork`
# return once the Space's cua-spacesd answers with its own token):
#   checkpoint       `cua sb checkpoint` of the running source
#   restore          `cua sb fork <checkpoint>`: a new Space restored from it
#   fork_running     `cua sb fork <running Space>` (checkpoint + restore)
#   fanout           FANOUT `cua sb fork <checkpoint>` at once: wall time
#   memory           host memory in use per running fork (and the guest's own)
#
# Raw samples go to $OUT/*.jsonl, the summary to $OUT/summary.json. Spaces
# and checkpoints it makes are named bench-vz-* and deleted at the end.
set -uo pipefail

CUA=${CUA:-cua}
IMAGE=${IMAGE:-ghcr.io/trycua/linux:24.04-slim}
N=${N:-5}
FANOUT=${FANOUT:-8}
CPUS=${CPUS:-2}
MEM=${MEM:-2GB}
SHM_MIB=${SHM_MIB:-256}
OUT=${OUT:-results}
mkdir -p "$OUT"
rm -f "$OUT"/*.jsonl
# Benchmarks are not product usage: no anonymous telemetry.
export DO_NOT_TRACK=1 CUA_TELEMETRY=0

now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }
log() { echo "[$(date +%H:%M:%S)] $*" >&2; }

# Host memory in use (active + wired + compressor), MiB.
host_used_mib() {
  vm_stat | python3 -c '
import re, sys
s = sys.stdin.read()
page = int(re.search(r"page size of (\d+)", s).group(1))
get = lambda k: int(re.search(k + r":\s+(\d+)", s).group(1))
print(round((get("Pages active") + get("Pages wired down") + get("Pages occupied by compressor")) * page / 1048576))'
}

# timed NAME FILE CMD... -> appends {"phase","name","ms","rc"}, prints ms
timed() {
  local name=$1 file=$2; shift 2
  local t0 t1 rc
  t0=$(now_ms)
  "$@" >/dev/null 2>"$OUT/$name.err"
  rc=$?
  t1=$(now_ms)
  echo "{\"phase\":\"$(basename "$file" .jsonl)\",\"name\":\"$name\",\"ms\":$((t1 - t0)),\"rc\":$rc}" >>"$file"
  if [ $rc -eq 0 ]; then rm -f "$OUT/$name.err"; else log "$name failed: $(tail -3 "$OUT/$name.err")"; fi
  echo $((t1 - t0))
}

rm_sb() { "$CUA" sb rm "$1" --force >/dev/null 2>&1 || true; }

cleanup_all() {
  for n in $("$CUA" sb ls --json 2>/dev/null | python3 -c 'import json,sys
try:
  d=json.load(sys.stdin)
except Exception:
  d=[]
rows=d if isinstance(d,list) else d.get("sandboxes",[])
print("\n".join(r.get("name","") for r in rows if r.get("name","").startswith("bench-vz-")))'); do
    rm_sb "$n"
  done
  for c in $("$CUA" sb checkpoints --json 2>/dev/null | python3 -c 'import json,sys
try:
  print("\n".join(c["name"] for c in json.load(sys.stdin) if c["name"].startswith("bench-vz-")))
except Exception:
  pass'); do
    "$CUA" sb checkpoints rm "$c" >/dev/null 2>&1 || true
  done
}
trap cleanup_all EXIT

log "source Space (the image's rootfs is built once, outside the samples)"
ms=$(timed bench-vz-src "$OUT/prepare.jsonl" "$CUA" sb create "$IMAGE" --name bench-vz-src \
  --kind microvm --cpus "$CPUS" --memory "$MEM")
[ -f "$OUT/bench-vz-src.err" ] && exit 1
log "source: ${ms} ms"
"$CUA" sb exec bench-vz-src -- sh -c "dd if=/dev/urandom of=/dev/shm/mem bs=1M count=$SHM_MIB 2>/dev/null; sync" \
  >/dev/null || { log "exec in the source failed"; exit 1; }

log "checkpoints, N=$N"
for i in $(seq 1 "$N"); do
  ms=$(timed "bench-vz-ckpt-$i" "$OUT/checkpoint.jsonl" "$CUA" sb checkpoint bench-vz-src "bench-vz-ckpt-$i")
  log "checkpoint $i: ${ms} ms"
done

log "restores (fork of a checkpoint), N=$N"
for i in $(seq 1 "$N"); do
  ms=$(timed "bench-vz-rst-$i" "$OUT/restore.jsonl" "$CUA" sb fork bench-vz-ckpt-1 "bench-vz-rst-$i")
  log "restore $i: ${ms} ms"
  rm_sb "bench-vz-rst-$i"
done

log "forks of the running source, N=$N"
for i in $(seq 1 "$N"); do
  ms=$(timed "bench-vz-fr-$i" "$OUT/fork_running.jsonl" "$CUA" sb fork bench-vz-src "bench-vz-fr-$i")
  log "fork of running $i: ${ms} ms"
  rm_sb "bench-vz-fr-$i"
done

log "fan-out: $FANOUT forks of one checkpoint at once"
used0=$(host_used_mib)
t0=$(now_ms)
pids=()
for i in $(seq 1 "$FANOUT"); do
  timed "bench-vz-fan-$i" "$OUT/fanout_each.jsonl" "$CUA" sb fork bench-vz-ckpt-1 "bench-vz-fan-$i" >/dev/null &
  pids+=($!)
done
for p in "${pids[@]}"; do wait "$p"; done
t1=$(now_ms)
ok=$(python3 -c "import json; print(sum(1 for l in open('$OUT/fanout_each.jsonl') if json.loads(l)['rc'] == 0))")
echo "{\"phase\":\"fanout\",\"name\":\"fanout-$FANOUT\",\"ms\":$((t1 - t0)),\"rc\":$([ "$ok" -eq "$FANOUT" ] && echo 0 || echo 1)}" >>"$OUT/fanout.jsonl"
log "fan-out: $FANOUT ready ($ok ok) in $((t1 - t0)) ms"
sleep 3
used1=$(host_used_mib)
guest=$("$CUA" sb exec bench-vz-fan-1 -- free -m 2>/dev/null | awk '/Mem:/ {print $3 + $5}')
echo "{\"phase\":\"memory\",\"forks\":$FANOUT,\"host_used_delta_mib\":$((used1 - used0)),\"per_fork_mib\":$(( (used1 - used0) / FANOUT )),\"guest_used_mib\":${guest:-0}}" >"$OUT/memory.json"
log "memory: host +$((used1 - used0)) MiB for $FANOUT forks; the guest uses ${guest:-?} MiB"

python3 - "$OUT" <<'PY'
import glob, json, os, statistics, sys
out = sys.argv[1]
def pct(v, p):
    v = sorted(v)
    if not v:
        return None
    k = (len(v) - 1) * p / 100
    f = int(k)
    c = min(f + 1, len(v) - 1)
    return round(v[f] + (v[c] - v[f]) * (k - f), 1)
res = {}
for f in sorted(glob.glob(os.path.join(out, "*.jsonl"))):
    rows = [json.loads(l) for l in open(f) if l.strip()]
    ok = [r["ms"] for r in rows if r.get("rc") == 0]
    res[os.path.basename(f)[:-6]] = {
        "n": len(rows), "ok": len(ok), "p50_ms": pct(ok, 50), "p95_ms": pct(ok, 95),
        "max_ms": max(ok) if ok else None,
        "mean_ms": round(statistics.mean(ok), 1) if ok else None,
    }
try:
    res["memory"] = json.load(open(os.path.join(out, "memory.json")))
except FileNotFoundError:
    pass
json.dump(res, open(os.path.join(out, "summary.json"), "w"), indent=2)
print(json.dumps(res, indent=2))
PY
