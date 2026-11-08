#!/bin/zsh
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Overnight comparison of Cua Driver builds under one harness, one model and one VM. Inside the benchmark VM, as user
# lume, from Terminal.app (so every claude process has a GUI session):  open -a Terminal overnight_compare.sh
#
#   overnight_compare.sh ROUNDS CUTOFF_UTC LABEL [LABEL ...]
#
# For every round r (0-based), condition (foreground, background) and LABEL it runs the ten v2 tasks once
# (one trial each; round r draws new task seeds through --run-offset r) with Claude Code + the build prepared as
# LABEL (tools/vm_main_build/assemble_version.sh; it needs ~/bench-work/versions/LABEL/{app,bin,cursor,skills,git_sha}).
# The order of the LABELs alternates every round and condition so time of night does not favour one build.
# No new trial starts after CUTOFF_UTC. Results: ~/bench-work/runs/ov-LABEL-COND-rN/results.jsonl. When it ends it
# writes ~/bench-work/overnight/DONE (and, if set, runs $CDB_AT_END), so a babysitter on another machine can collect
# the results, delete the token and release the VM slot.
export CDB_BENCH_WORK=$HOME/bench-work CDB_BENCH_DISPOSABLE=1 CDB_TASKPACK=$HOME/cdb-runtime CDB_EVAL_SUDO=1
export CDB_CLAUDE_TOKEN_FILE=$HOME/.cdb-secrets/claude-token
ROUNDS=${1:?rounds}; CUTOFF=${2:?cutoff utc}; shift 2
LABELS=("$@")
OUT=$CDB_BENCH_WORK/overnight; mkdir -p $OUT; rm -f $OUT/DONE
HERE=${0:A:h}; BENCH=${HERE:h}
PY=/opt/homebrew/bin/python3; [ -x "$PY" ] || PY="$(command -v python3)"
cd $BENCH || exit 1
/usr/bin/caffeinate -dimsu -w $$ &
log() { echo "$(date -u +%FT%TZ) $*" >> $OUT/overnight.log; }
past_cutoff() { [[ "$(date -u +%FT%TZ)" > "$CUTOFF" ]]; }
log "start rounds=$ROUNDS cutoff=$CUTOFF labels=$LABELS"
n=0
for ((r = 0; r < ROUNDS; r++)); do
  ci=0
  for cond in foreground background; do
    ((ci++))
    order=("${LABELS[@]}")
    if (( (r + ci) % 2 )); then order=("${(Oa)LABELS[@]}"); fi
    for ver in "${order[@]}"; do
      past_cutoff && { log "cutoff reached"; break 3; }
      V=$CDB_BENCH_WORK/versions/$ver
      GIT_SHA=$(cat $V/git_sha) SIGN_IDENTITY=${SIGN_IDENTITY:--} \
        zsh tools/vm_main_build/assemble_version.sh $V/app $V/bin $V/cursor $V/skills $ver >> $OUT/assemble.log 2>&1 \
        || { log "assemble $ver failed"; continue; }
      RUN=ov-$ver-$cond-r$r
      log "run $RUN"
      $PY run_bench.py run --bench-v2 --condition $cond --arms cc-cua-driver-script --task-set v2 \
        --phase1-runs 1 --phase2-runs 0 --run-offset $r --run-id $RUN \
        --pins $CDB_BENCH_WORK/pins-$ver.json --build-dir $CDB_BENCH_WORK/build \
        --cutoff-utc "$CUTOFF" >> $CDB_BENCH_WORK/runs-$RUN.log 2>&1
      log "run $RUN exit $?"
      ((n++))
    done
  done
done
log "finished after $n runs"
[ -n "$CDB_AT_END" ] && eval "$CDB_AT_END"
date -u +%FT%TZ > $OUT/DONE
