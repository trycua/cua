#!/bin/bash
# Every 15 s, append the pool coordinator's counters, metrics and per-machine load (JSON lines).
export DO_NOT_TRACK=1 CUA_HOME=${CUA_HOME:-$HOME/.cua}
D=${1:-4200}; OUT=${2:-$HOME/coord-samples.jsonl}
END=$(( $(date +%s) + D ))
while [ "$(date +%s)" -lt "$END" ]; do
  cua computers --json 2>/dev/null | python3 -c '
import json,sys,time
d=json.load(sys.stdin)
m=d.get("metrics",{})
out={"ts":int(time.time()),"in_flight":d.get("in_flight"),"jobs":d.get("jobs"),
     "per_sec_60s":m.get("per_sec_60s"),"dup_suppressed":m.get("duplicates_suppressed"),
     "retries":m.get("retries"),"node_down":m.get("node_down"),"fences":m.get("fences"),
     "dispatch_p50_ms":m.get("dispatch_p50_ms"),"dispatch_p99_ms":m.get("dispatch_p99_ms"),
     "nodes":{n.get("name"):[n.get("jobs_running"),n.get("health")] for n in d.get("machines",[])}}
print(json.dumps(out))' >> $OUT 2>/dev/null
  sleep 15
done
