#!/usr/bin/env python3
"""analyze.py RUN_ID... — summarize browser-session runs from `cua sb runs RUN --json`."""
import json, os, subprocess, sys, collections, base64

env = dict(os.environ, DO_NOT_TRACK="1")

def jobs(run):
    out = subprocess.run(["cua", "sb", "runs", run, "--json"], capture_output=True, text=True, env=env).stdout
    return json.loads(out)["jobs"]

def q(a, p):
    a = sorted(a)
    return a[min(len(a) - 1, int(p * len(a)))] if a else None

for run in sys.argv[1:]:
    J = jobs(run)
    states = collections.Counter(j.get("state") for j in J)
    ok = [j for j in J if j.get("state") == "succeeded"]
    sess = []
    import concurrent.futures as cf
    def full(j):
        o = subprocess.run(["cua", "sb", "runs", run, "--job", str(j["id"]), "--json"], capture_output=True, text=True, env=env).stdout
        try:
            return json.loads(o)
        except Exception:
            return j
    import random
    sample = ok
    n_sample = int(os.environ.get("SAMPLE", "0"))
    if n_sample and len(ok) > n_sample:
        sample = random.Random(7).sample(ok, n_sample)
    with cf.ThreadPoolExecutor(16) as ex:
        ok_full = list(ex.map(full, sample))
    for j in ok_full:
        out = j.get("stdout") or ""
        if isinstance(out, list):
            out = bytes(out).decode(errors="replace")
        if not out and j.get("stdout_b64"):
            out = base64.b64decode(j["stdout_b64"]).decode(errors="replace")
        line = [l for l in out.strip().splitlines() if l.startswith("{")]
        if line:
            try:
                sess.append(json.loads(line[-1]))
            except Exception:
                pass
    fin = sorted(j["finished_ms"] for j in ok if j.get("finished_ms"))
    span = (fin[-1] - fin[0]) / 1000 if len(fin) > 1 else 0
    by_node = collections.Counter(j.get("node") for j in ok)
    errs = collections.Counter((j.get("error") or "")[:90] for j in J if j.get("state") in ("error", "failed"))
    print(json.dumps({
        "run": run, "jobs": len(J), "states": states,
        "space_start_ms_p50": q([j["create_ms"] for j in ok if j.get("create_ms") is not None], .5),
        "space_start_ms_p95": q([j["create_ms"] for j in ok if j.get("create_ms") is not None], .95),
        "run_ms_p50": q([j["run_ms"] for j in ok if j.get("run_ms")], .5),
        "delete_ms_p50": q([j["delete_ms"] for j in ok if j.get("delete_ms") is not None], .5),
        "sessions_parsed": len(sess),
        "pages": sum(s.get("pages", 0) for s in sess),
        "pages_rendered": sum(s.get("rendered", 0) for s in sess),
        "sessions_with_render": sum(1 for s in sess if s.get("rendered", 0) > 0),
        "completed_per_hour_over_span": round(len(fin) / span * 3600) if span else None,
        "by_node": by_node, "top_errors": errs.most_common(4),
        "keys": sorted(ok[0].keys()) if ok else None,
    }, default=str))
