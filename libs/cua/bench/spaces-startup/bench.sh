#!/usr/bin/env bash
# Spaces startup, snapshot and density benchmark, through the real `cua` CLI
# and daemon (the same path a user or an agent takes).
#
# Usage (on a Linux host with KVM for the microVM kind):
#   CUA=/path/to/cua OUT=results/ bash bench.sh [all|microvm|gvisor|density]
#
# Environment:
#   CUA        the cua binary (default: cua on PATH)
#   IMAGE      Space image (default ghcr.io/trycua/linux:24.04-slim)
#   N          samples per start measurement (default 20)
#   NCKPT      checkpoint/fork samples (default 10)
#   DENSITY_MAX, DENSITY_STEP, DENSITY_FLOOR_MB  density ramp: start Spaces
#              STEP at a time (up to MAX) until MemAvailable drops below
#              FLOOR_MB (defaults 400, 10, 4096)
#   CHROMIUM=1 also open a headless Chromium page in every density Space
#   DENSITY_KINDS  which kinds the density phase ramps (default "microvm gvisor")
#   CPUS, MEM  Space size (default 2 vCPU, 2GB)
#
# VM density (`vm-density`, macOS with HVF or Linux with KVM): QEMU `vm`
# Spaces started VMD_STEP at a time through the SDK (rollout_bench, the path
# a trainer takes) until one fails or is refused (HostCapacity), the exec p95
# goes over 2x its value with one VM, or memory pressure turns critical:
#   ROLLOUT_BENCH  the rollout_bench example binary (cargo build --release -p
#              cua-sdk --example rollout_bench); required
#   VMD_CPUS, VMD_MEM_MB  VM size (default 1 vCPU, 1024 MiB)
#   VMD_MAX, VMD_STEP  most VMs and VMs per step (default 512, 8)
#   VMD_HEADLESS=1  only cua-spacesd in the guest (no desktop services)
#   VMD_PROFILE  a size profile (`rollout`: 1 vCPU, 1 GiB, no desktop and no
#              display hardware); VMD_CPUS / VMD_MEM_MB still win
#   VMD_LABEL  names the run (default the host's memory and CPUs)
# Each step appends a sample (sample_host.py: host memory, pressure, the
# VMs' private memory plus shared snapshot memory counted once) and an exec
# probe to $OUT/vm-density-<label>.jsonl; the summary line has the most VMs,
# memory per VM, the stop reason and an extrapolation to 64 and 128 GB.
# `guard` compares a summary with a committed baseline (baseline/*.json) and
# fails past its thresholds.
#
# What it measures (wall time of each CLI command, which returns once the
# Space's cua-spacesd answers with its own token):
#   cold    microVM start with warm snapshots off (kernel boot to ready)
#   warm    microVM start restored from the golden snapshot
#   ckpt    `cua sb checkpoint` of a running microVM Space
#   fork    `cua sb fork` from a checkpoint and from a running Space
#   gvisor  gVisor container start (image cached), for comparison
#   density concurrently running Spaces and memory per Space
#
# Raw samples go to $OUT/*.jsonl, a summary to $OUT/summary.json. The daemon
# is restarted with the settings each phase needs; Spaces it creates are
# named bench-* and deleted at the end of each phase.
set -uo pipefail

CUA=${CUA:-cua}
IMAGE=${IMAGE:-ghcr.io/trycua/linux:24.04-slim}
N=${N:-20}
NCKPT=${NCKPT:-10}
CPUS=${CPUS:-2}
MEM=${MEM:-2GB}
OUT=${OUT:-results}
DENSITY_MAX=${DENSITY_MAX:-400}
DENSITY_STEP=${DENSITY_STEP:-10}
DENSITY_FLOOR_MB=${DENSITY_FLOOR_MB:-4096}
WHAT=${1:-all}
mkdir -p "$OUT"
# Benchmarks are not product usage: no anonymous telemetry (its upload at
# exit also adds a network round trip to every CLI command).
export DO_NOT_TRACK=1 CUA_TELEMETRY=0

# Milliseconds since the epoch: GNU date has %3N; BSD date (macOS) does not.
if [ -n "$(date +%s%3N | tr -d 0-9)" ]; then
  now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }
else
  now_ms() { date +%s%3N; }
fi
log() { echo "[$(date +%H:%M:%S)] $*" >&2; }

daemon_restart() {
  "$CUA" daemon stop >/dev/null 2>&1 || true
  sleep 1
  # This build's daemon, in the foreground (`daemon start` may hand over to
  # an installed app's daemon of the same version); the SDK starts this one
  # too (CUA_BIN).
  export CUA_BIN="$CUA"
  # shellcheck disable=SC2068
  env "$@" nohup "$CUA" daemon start --foreground >>"$OUT/daemon.log" 2>&1 &
  for _ in $(seq 1 300); do "$CUA" daemon status >/dev/null 2>&1 && return 0; sleep 0.1; done
  log "daemon did not come up: $(tail -3 "$OUT/daemon.log" 2>/dev/null)"; exit 1
}

# create NAME KIND_ARGS... -> prints ms, appends a sample
timed_create() {
  local file=$1 name=$2; shift 2
  local t0 t1 rc
  t0=$(now_ms)
  "$CUA" sb create "$IMAGE" --name "$name" --cpus "$CPUS" --memory "$MEM" "$@" >/dev/null 2>"$OUT/$name.err"
  rc=$?
  t1=$(now_ms)
  echo "{\"phase\":\"$(basename "$file" .jsonl)\",\"name\":\"$name\",\"ms\":$((t1 - t0)),\"rc\":$rc}" >>"$file"
  [ $rc -eq 0 ] && rm -f "$OUT/$name.err"
  if [ $rc -ne 0 ] && [ "$(basename "$file")" = prepare.jsonl ]; then
    log "preparing $name failed: $(tail -3 "$OUT/$name.err")"
    exit 1
  fi
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
print("\n".join(r.get("name","") for r in rows if r.get("name","").startswith("bench-")))'); do
    rm_sb "$n"
  done
  for c in $("$CUA" sb checkpoints --json 2>/dev/null | python3 -c 'import json,sys
try:
  print("\n".join(c["name"] for c in json.load(sys.stdin) if c["name"].startswith("bench-")))
except Exception:
  pass'); do
    "$CUA" sb checkpoints rm "$c" >/dev/null 2>&1 || true
  done
}

microvm_phase() {
  log "microVM cold starts (warm snapshots off), N=$N"
  daemon_restart CUA_MICROVM_WARM=0
  # The rootfs and kernel are built/downloaded once, outside the samples.
  timed_create "$OUT/prepare.jsonl" bench-prep --kind microvm >/dev/null
  rm_sb bench-prep
  for i in $(seq 1 "$N"); do
    ms=$(timed_create "$OUT/microvm-cold.jsonl" "bench-cold-$i" --kind microvm)
    log "cold $i: ${ms} ms"
    rm_sb "bench-cold-$i"
  done

  log "microVM warm starts (golden snapshot), N=$N"
  daemon_restart CUA_MICROVM_WARM=1
  timed_create "$OUT/prepare.jsonl" bench-seed --kind microvm >/dev/null  # records the golden snapshot
  rm_sb bench-seed
  for i in $(seq 1 "$N"); do
    ms=$(timed_create "$OUT/microvm-warm.jsonl" "bench-warm-$i" --kind microvm)
    log "warm $i: ${ms} ms"
    rm_sb "bench-warm-$i"
  done

  log "checkpoint and fork, N=$NCKPT"
  timed_create "$OUT/prepare.jsonl" bench-src --kind microvm >/dev/null
  for i in $(seq 1 "$NCKPT"); do
    t0=$(now_ms)
    "$CUA" sb checkpoint bench-src "bench-ckpt-$i" >/dev/null 2>>"$OUT/errors.log"
    rc=$?
    t1=$(now_ms)
    echo "{\"phase\":\"microvm-checkpoint\",\"name\":\"bench-ckpt-$i\",\"ms\":$((t1 - t0)),\"rc\":$rc}" >>"$OUT/microvm-checkpoint.jsonl"
    log "checkpoint $i: $((t1 - t0)) ms"
  done
  for i in $(seq 1 "$NCKPT"); do
    t0=$(now_ms)
    "$CUA" sb fork "bench-ckpt-1" "bench-forkc-$i" >/dev/null 2>>"$OUT/errors.log"
    rc=$?
    t1=$(now_ms)
    echo "{\"phase\":\"microvm-fork-checkpoint\",\"name\":\"bench-forkc-$i\",\"ms\":$((t1 - t0)),\"rc\":$rc}" >>"$OUT/microvm-fork-checkpoint.jsonl"
    log "fork from checkpoint $i: $((t1 - t0)) ms"
    rm_sb "bench-forkc-$i"
  done
  for i in $(seq 1 "$NCKPT"); do
    t0=$(now_ms)
    "$CUA" sb fork bench-src "bench-forkr-$i" >/dev/null 2>>"$OUT/errors.log"
    rc=$?
    t1=$(now_ms)
    echo "{\"phase\":\"microvm-fork-running\",\"name\":\"bench-forkr-$i\",\"ms\":$((t1 - t0)),\"rc\":$rc}" >>"$OUT/microvm-fork-running.jsonl"
    log "fork of running $i: $((t1 - t0)) ms"
    rm_sb "bench-forkr-$i"
  done
  cleanup_all
}

# Kernel boot to a ready service on a small image (no desktop): what a
# microVM cold boot costs before the guest's own userland.
MINIMAL_IMAGE=${MINIMAL_IMAGE:-nginx:alpine}
minimal_phase() {
  log "minimal image ($MINIMAL_IMAGE) cold and warm microVM starts, N=$N"
  local img_save=$IMAGE
  IMAGE=$MINIMAL_IMAGE
  daemon_restart CUA_MICROVM_WARM=0
  timed_create "$OUT/prepare.jsonl" bench-mprep --kind microvm --port 80 --wait http:80/ >/dev/null
  rm_sb bench-mprep
  for i in $(seq 1 "$N"); do
    ms=$(timed_create "$OUT/minimal-cold.jsonl" "bench-mcold-$i" --kind microvm --port 80 --wait http:80/)
    log "minimal cold $i: ${ms} ms"
    rm_sb "bench-mcold-$i"
  done
  daemon_restart CUA_MICROVM_WARM=1
  timed_create "$OUT/prepare.jsonl" bench-mseed --kind microvm --port 80 --wait http:80/ >/dev/null
  rm_sb bench-mseed
  for i in $(seq 1 "$N"); do
    ms=$(timed_create "$OUT/minimal-warm.jsonl" "bench-mwarm-$i" --kind microvm --port 80 --wait http:80/)
    log "minimal warm $i: ${ms} ms"
    rm_sb "bench-mwarm-$i"
  done
  for i in $(seq 1 "$N"); do
    ms=$(timed_create "$OUT/minimal-gvisor.jsonl" "bench-mgv-$i" --kind container --runtime gvisor --port 80 --wait http:80/)
    log "minimal gvisor $i: ${ms} ms"
    rm_sb "bench-mgv-$i"
  done
  IMAGE=$img_save
  cleanup_all
}

gvisor_phase() {
  log "gVisor starts, N=$N (image cached after the first)"
  daemon_restart
  timed_create "$OUT/prepare.jsonl" bench-gprep --kind container --runtime gvisor >/dev/null
  rm_sb bench-gprep
  for i in $(seq 1 "$N"); do
    ms=$(timed_create "$OUT/gvisor-start.jsonl" "bench-gv-$i" --kind container --runtime gvisor)
    log "gvisor $i: ${ms} ms"
    rm_sb "bench-gv-$i"
  done
  log "gVisor checkpoint, N=$NCKPT"
  timed_create "$OUT/prepare.jsonl" bench-gsrc --kind container --runtime gvisor >/dev/null
  for i in $(seq 1 "$NCKPT"); do
    t0=$(now_ms)
    "$CUA" sb checkpoint bench-gsrc "bench-gckpt-$i" >/dev/null 2>>"$OUT/errors.log"
    rc=$?
    t1=$(now_ms)
    echo "{\"phase\":\"gvisor-checkpoint\",\"name\":\"bench-gckpt-$i\",\"ms\":$((t1 - t0)),\"rc\":$rc}" >>"$OUT/gvisor-checkpoint.jsonl"
    log "gvisor checkpoint $i: $((t1 - t0)) ms"
  done
  for i in $(seq 1 "$NCKPT"); do
    t0=$(now_ms)
    "$CUA" sb fork "bench-gckpt-1" "bench-gfork-$i" >/dev/null 2>>"$OUT/errors.log"
    rc=$?
    t1=$(now_ms)
    echo "{\"phase\":\"gvisor-fork-checkpoint\",\"name\":\"bench-gfork-$i\",\"ms\":$((t1 - t0)),\"rc\":$rc}" >>"$OUT/gvisor-fork-checkpoint.jsonl"
    log "gvisor fork $i: $((t1 - t0)) ms"
    rm_sb "bench-gfork-$i"
  done
  cleanup_all
}

mem_available_mb() { awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo; }

# Memory of what runs the Spaces of one kind: PSS of every firecracker and
# pasta process (microVM), or of every runsc process (gVisor).
pss_mb() {
  local pattern=$1
  local total=0
  for p in $(pgrep -f "$pattern"); do
    v=$(awk '/^Pss:/ {print $2}' "/proc/$p/smaps_rollup" 2>/dev/null)
    total=$((total + ${v:-0}))
  done
  echo $((total / 1024))
}

chromium_in() {
  "$CUA" sb exec "$1" "setsid -f chromium --headless=new --no-sandbox --disable-gpu --user-data-dir=/tmp/bench-chromium --remote-debugging-port=9222 https://example.com >/tmp/chromium.log 2>&1 </dev/null" >/dev/null 2>&1
}

density_kind() {
  local label=$1; shift
  local pattern=$1; shift
  local base_avail
  base_avail=$(mem_available_mb)
  local n=0
  log "density $label: MemAvailable ${base_avail} MiB at start"
  while [ "$n" -lt "$DENSITY_MAX" ]; do
    local batch=()
    for _ in $(seq 1 "$DENSITY_STEP"); do
      n=$((n + 1))
      batch+=("bench-d$label-$n")
    done
    local t0
    t0=$(now_ms)
    for b in "${batch[@]}"; do
      ( "$CUA" sb create "$IMAGE" --name "$b" --cpus "$CPUS" --memory "$MEM" "$@" >/dev/null 2>>"$OUT/errors.log" \
          && { [ "${CHROMIUM:-0}" = 1 ] && chromium_in "$b"; true; } ) &
    done
    wait
    local t1 avail running pss
    t1=$(now_ms)
    sleep "${SETTLE:-10}"
    avail=$(mem_available_mb)
    running=$("$CUA" sb ls --json 2>/dev/null | python3 -c "import json,sys
d=json.load(sys.stdin); rows=d if isinstance(d,list) else d.get('sandboxes',[])
print(sum(1 for r in rows if r.get('name','').startswith('bench-d$label-') and str(r.get('state','')).lower() in ('running','sandbox_state_running','2')))" 2>/dev/null || echo 0)
    pss=$(pss_mb "$pattern")
    echo "{\"phase\":\"density-$label\",\"chromium\":${CHROMIUM:-0},\"started\":$n,\"running\":$running,\"batch_ms\":$((t1 - t0)),\"mem_available_mb\":$avail,\"mem_used_mb\":$((base_avail - avail)),\"pss_mb\":$pss}" >>"$OUT/density-$label.jsonl"
    log "density $label: started $n running $running, MemAvailable ${avail} MiB, used $((base_avail - avail)) MiB, PSS ${pss} MiB, batch $((t1 - t0)) ms"
    if [ "$avail" -lt "$DENSITY_FLOOR_MB" ] || [ "$running" -lt "$((n - DENSITY_STEP))" ]; then
      break
    fi
  done
  cleanup_all
}

density_phase() {
  daemon_restart CUA_MICROVM_WARM=1
  timed_create "$OUT/prepare.jsonl" bench-dseed --kind microvm >/dev/null
  rm_sb bench-dseed
  for k in ${DENSITY_KINDS:-microvm gvisor}; do
    case $k in
      microvm) density_kind microvm "firecracker|pasta" --kind microvm ;;
      gvisor) density_kind gvisor "runsc" --kind container --runtime gvisor ;;
    esac
  done
}

summary() {
  python3 - "$OUT" <<'PY'
import glob, json, os, statistics, sys
out = sys.argv[1]
res = {}
def pct(v, p):
    v = sorted(v)
    if not v:
        return None
    k = (len(v) - 1) * p / 100
    f = int(k)
    c = min(f + 1, len(v) - 1)
    return round(v[f] + (v[c] - v[f]) * (k - f), 1)
for f in sorted(glob.glob(os.path.join(out, "*.jsonl"))):
    rows = [json.loads(l) for l in open(f) if l.strip()]
    name = os.path.basename(f)[:-6]
    if name.startswith("density"):
        res[name] = rows[-1] if rows else None
        if rows:
            last = rows[-1]
            res[name]["mib_per_space"] = round(last["mem_used_mb"] / max(last["running"], 1), 1)
        continue
    ok = [r["ms"] for r in rows if r.get("rc") == 0]
    res[name] = {"n": len(rows), "ok": len(ok), "p50_ms": pct(ok, 50), "p95_ms": pct(ok, 95),
                 "min_ms": min(ok) if ok else None, "mean_ms": round(statistics.mean(ok), 1) if ok else None}
json.dump(res, open(os.path.join(out, "summary.json"), "w"), indent=2)
print(json.dumps(res, indent=2))
PY
}

# One sample of the host and its QEMU VMs (sample_host.py).
host_sample() { python3 "$(dirname "$0")/sample_host.py" "$1"; }

vm_density_phase() {
  [ -x "${ROLLOUT_BENCH:-}" ] || { log "set ROLLOUT_BENCH to the rollout_bench binary"; exit 2; }
  local cpus=${VMD_CPUS:-1} mem=${VMD_MEM_MB:-1024} max=${VMD_MAX:-512} step=${VMD_STEP:-8}
  local hl=() label prefix=bench-vmd-
  [ "${VMD_HEADLESS:-0}" = 1 ] && hl=(--headless)
  [ -n "${VMD_PROFILE:-}" ] && hl+=(--profile "$VMD_PROFILE")
  # A local disk (IMAGE=disk:...): wait for its cua-spacesd, as the daemon
  # does for a catalog image (and records its warm snapshot then).
  case "$IMAGE" in disk:*) hl+=(--wait-tcp 3211) ;; esac
  label=${VMD_LABEL:-$(python3 -c 'import json,sys; sys.path.insert(0, sys.argv[1]); import sample_host as s; h=s.host(); import os; print(f"{h[\"total_mb\"]//1024}g-{os.cpu_count()}c")' "$(dirname "$0")")}
  local f="$OUT/vm-density-$label.jsonl"
  : >"$f"
  daemon_restart
  log "vm density $label: ${cpus} vCPU / ${mem} MiB VMs, ${step} per step, at most $max"
  log "daemon: $(python3 -c 'import json,os,sys; print(json.load(open(os.path.join(sys.argv[1],"daemon.json")))["pid"])' "${CUA_HOME:-$HOME/.cua}" 2>/dev/null) (this build: $CUA)"
  host_sample idle | sed 's/^{/{"phase":"idle","running":0,/' >>"$f"
  # The first VM records the golden snapshot every later one restores.
  "$ROLLOUT_BENCH" create --prefix "${prefix}0-" --n 1 --image "$IMAGE" --kind vm --cpus "$cpus" --memory-mb "$mem" \
    "${hl[@]}" --keep --label "$label-first" >"$OUT/vmd.tmp" 2>>"$OUT/errors.log"
  grep summary "$OUT/vmd.tmp" | sed 's/^{/{"phase":"create","target":1,/' >>"$f"
  local n=1 base="" stop="max"
  while :; do
    sleep "${SETTLE:-10}"
    host_sample "n$n" | sed "s/^{/{\"phase\":\"host\",\"running\":$n,/" >>"$f"
    "$ROLLOUT_BENCH" probe --prefix "$prefix" --iters 20 --conc 1 --label "$label-n$n" \
      2>>"$OUT/errors.log" | sed "s/^{/{\"phase\":\"probe\",\"running\":$n,/" >>"$f"
    local p95 errs pressure
    p95=$(tail -1 "$f" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("p95", 0))')
    errs=$(tail -1 "$f" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("errors", 1))')
    pressure=$(tail -2 "$f" | head -1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["host"]["pressure"])')
    [ -z "$base" ] && base=$p95
    log "vm density $label: $n running, exec p95 ${p95} ms (one VM: ${base} ms), pressure $pressure"
    if [ "$errs" != 0 ]; then stop="probe errors ($errs)"; break; fi
    if python3 -c "import sys; sys.exit(0 if $p95 > 2 * $base and $p95 > $base + 5 else 1)"; then
      stop="exec p95 over 2x"; break
    fi
    if [ "$pressure" = critical ]; then stop="memory pressure critical"; break; fi
    [ "$n" -ge "$max" ] && break
    local k=$(( max - n < step ? max - n : step ))
    "$ROLLOUT_BENCH" create --prefix "${prefix}$n-" --n "$k" --conc "$k" --image "$IMAGE" --kind vm --cpus "$cpus" \
      --memory-mb "$mem" "${hl[@]}" --keep --label "$label-to$((n + k))" >"$OUT/vmd.tmp" 2>>"$OUT/errors.log"
    grep summary "$OUT/vmd.tmp" | sed "s/^{/{\"phase\":\"create\",\"target\":$((n + k)),/" >>"$f"
    local ok failed refused
    ok=$(grep -c '"create_ms"' "$OUT/vmd.tmp" || true)
    failed=$(grep summary "$OUT/vmd.tmp" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("failed", 0))')
    refused=$(grep -c -i 'host capacity\|HostCapacity\|memory limit\|CPU limit' "$OUT/vmd.tmp" "$OUT/errors.log" 2>/dev/null | awk -F: '{s+=$2} END {print s+0}')
    n=$(( n + k - failed ))
    if [ "$failed" != 0 ]; then
      stop=$([ "$refused" -gt 0 ] && echo "admission refused (HostCapacity)" || echo "create failed ($failed)")
      sleep "${SETTLE:-10}"
      host_sample "n$n" | sed "s/^{/{\"phase\":\"host\",\"running\":$n,/" >>"$f"
      break
    fi
  done
  python3 - "$f" "$stop" "$cpus" "$mem" <<'PY' | tee -a "$f"
import json, sys
f, stop, cpus, mem = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
rows = [json.loads(l) for l in open(f) if l.strip()]
hosts = [r for r in rows if r.get("phase") == "host" and r.get("vms")]
probes = [r for r in rows if r.get("phase") == "probe"]
idle = next(r for r in rows if r.get("phase") == "idle")
last = hosts[-1]
n = last["vms"]
per_vm = last["vm_rss_mb"] / max(n, 1)
total = last["host"]["total_mb"]
# Host overhead outside the VMs (the OS, the daemon): what the idle host
# already used.
base_used = total - idle["host"]["available_mb"]
import subprocess
hv = subprocess.run(["sysctl", "-n", "kern.hv.max_address_spaces"], capture_output=True, text=True).stdout.strip()
hv_max = int(hv) if hv.isdigit() else None
def fit(gb, frac=0.8):
    cap = gb * 1024 * frac
    return int((cap - last["vm_shared_mb"]) / max(last["vm_private_p50_mb"], 1))
print(json.dumps({
    "summary": True, "phase": "vm-density", "stop": stop, "vcpus": cpus, "memory_mb": mem,
    "max_running": n, "host_total_mb": total, "host_cpus": __import__("os").cpu_count(),
    "per_vm_mb": round(per_vm, 1), "vm_private_p50_mb": last["vm_private_p50_mb"],
    "vm_shared_mb": last["vm_shared_mb"], "vm_rss_mb": last["vm_rss_mb"],
    "vm_rss_sum_mb": last["vm_rss_sum_mb"],
    "available_mb_at_max": last["host"]["available_mb"], "pressure_at_max": last["host"]["pressure"],
    "compressed_mb_at_max": last["host"].get("compressed_mb", 0),
    "exec_p95_ms_one": probes[0]["p95"] if probes else None,
    "exec_p95_ms_max": probes[-1]["p95"] if probes else None,
    "exec_p50_ms_max": probes[-1]["p50"] if probes else None,
    "host_used_idle_mb": base_used,
    # Memory alone, at the default cap (0.8 of physical memory), with this
    # run's per-VM private memory and shared snapshot memory.
    "fit_by_memory": {"this": fit(total / 1024), "64g": fit(64), "128g": fit(128)},
    # What else caps a host: macOS Hypervisor.framework's VM limit
    # (kern.hv.max_address_spaces, 128 on current Macs) and the vCPU
    # overcommit (default 8x the host's CPUs).
    "hypervisor_max_vms": hv_max,
}))
PY
  "$ROLLOUT_BENCH" delete --prefix "$prefix" --conc 16 >/dev/null 2>>"$OUT/errors.log" || true
  cleanup_all
  "$CUA" daemon stop >/dev/null 2>&1 || true
}

# guard SUMMARY BASELINE: fail when SUMMARY is past BASELINE's thresholds.
guard() {
  python3 - "$1" "$2" <<'PY'
import json, sys
cur = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
cur = [r for r in cur if r.get("summary")][-1]
base = json.load(open(sys.argv[2]))
t = base["thresholds"]
fails = []
if cur["max_running"] < base["max_running"] * t["min_running_ratio"]:
    fails.append(f"max_running {cur['max_running']} < {t['min_running_ratio']} x {base['max_running']}")
if cur["vm_private_p50_mb"] > base["vm_private_p50_mb"] * t["max_private_ratio"]:
    fails.append(f"private per VM {cur['vm_private_p50_mb']} MiB > {t['max_private_ratio']} x {base['vm_private_p50_mb']}")
if cur["exec_p95_ms_one"] and cur["exec_p95_ms_one"] > base["exec_p95_ms_one"] * t["max_exec_ratio"]:
    fails.append(f"exec p95 with one VM {cur['exec_p95_ms_one']} ms > {t['max_exec_ratio']} x {base['exec_p95_ms_one']}")
print(json.dumps({"guard": "vm-density", "baseline": sys.argv[2], "current": cur, "failures": fails}))
sys.exit(1 if fails else 0)
PY
}

case "$WHAT" in
  vm-density) vm_density_phase; exit 0 ;;
  guard) guard "$2" "$3"; exit $? ;;
  microvm) microvm_phase ;;
  minimal) minimal_phase ;;
  gvisor) gvisor_phase ;;
  density) density_phase ;;
  summary) ;;
  all) microvm_phase; gvisor_phase; minimal_phase; density_phase ;;
  *) echo "usage: bench.sh [all|microvm|gvisor|minimal|density|vm-density|guard SUMMARY BASELINE|summary]" >&2; exit 2 ;;
esac
summary
