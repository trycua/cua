#!/usr/bin/env python3
"""window.py SAMPLES.jsonl START_TS END_TS — throughput and concurrency over a window of coordinator samples."""
import json, sys, statistics as st
path, t0, t1 = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
S = [json.loads(l) for l in open(path) if l.strip()]
W = [s for s in S if t0 <= s["ts"] <= t1]
a, b = W[0], W[-1]
done = lambda s: s["jobs"].get("succeeded", 0)
fail = lambda s: s["jobs"].get("error", 0) + s["jobs"].get("failed", 0)
hours = (b["ts"] - a["ts"]) / 3600
nodes = {}
for s in W:
    for n, (r, h) in s["nodes"].items():
        nodes.setdefault(n, []).append(r or 0)
print(json.dumps({
    "window_min": round((b["ts"] - a["ts"]) / 60, 1),
    "succeeded": done(b) - done(a),
    "failed_or_error": fail(b) - fail(a),
    "succeeded_per_hour": round((done(b) - done(a)) / hours) if hours else None,
    "in_flight_avg": round(st.mean(s["in_flight"] for s in W)),
    "in_flight_max": max(s["in_flight"] for s in W),
    "dup_suppressed": b.get("dup_suppressed"), "retries_delta": (b.get("retries") or 0) - (a.get("retries") or 0),
    "node_down_delta": (b.get("node_down") or 0) - (a.get("node_down") or 0),
    "per_node_avg_running": {n: round(st.mean(v)) for n, v in sorted(nodes.items())},
}, indent=1))
