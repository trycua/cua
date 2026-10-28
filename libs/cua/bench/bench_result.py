#!/usr/bin/env python3
"""bench-result v1: one JSON file per benchmark run, plus a CSV of its samples.

Every bench under libs/cua/bench writes its own native output (JSON lines,
a summary JSON, a guard JSON). The converters here turn each native output
into the shared shape (README.md, "The result format") so one reader (a
chart, a regression report, a web page) reads every bench the same way.

A converter only reads numbers the bench measured. It raises `NoResult`
when the native output holds no measured sample, and nothing is written
then: an unmeasured result never exists.

    from bench_result import convert, write
    core, samples = convert("microvm-fork", "/path/to/raw")
    write(out_dir, finish(core, ...), samples)
"""

from __future__ import annotations

import csv
import glob
import json
import math
import os
from typing import Any, Callable, Dict, Iterable, List, Optional, Tuple

SCHEMA = "cua.bench-result/v1"

Samples = List[Dict[str, Any]]
Core = Dict[str, Any]


class NoResult(Exception):
    """The native output has nothing measured to report."""


# ------------------------------------------------------------------ helpers


def pct(values: Iterable[float], p: float) -> Optional[float]:
    """Linear-interpolated percentile (the same rule as the benches' own
    summaries), rounded to 0.1."""
    v = sorted(float(x) for x in values)
    if not v:
        return None
    k = (len(v) - 1) * p / 100
    f = int(math.floor(k))
    c = min(f + 1, len(v) - 1)
    return round(v[f] + (v[c] - v[f]) * (k - f), 1)


def percentiles(values: Iterable[float]) -> Dict[str, float]:
    v = [float(x) for x in values]
    if not v:
        return {}
    out = {f"p{p}": pct(v, p) for p in (50, 90, 95, 99)}
    out["min"] = round(min(v), 1)
    out["max"] = round(max(v), 1)
    return out


def _num(x: Any) -> bool:
    return isinstance(x, (int, float)) and not isinstance(x, bool) and math.isfinite(float(x))


def numbers(d: Dict[str, Any]) -> Dict[str, float]:
    """The numeric fields of a dict (`extra` holds numbers only)."""
    return {k: v for k, v in d.items() if _num(v)}


def read_jsonl(path: str) -> List[dict]:
    rows = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    return rows


def indexed(rows: List[dict]) -> Samples:
    """Rows with `i` as their first column, counting from 1."""
    return [{"i": n, **{k: v for k, v in r.items() if k != "i"}} for n, r in enumerate(rows, 1)]


def core(metric: str, unit: str, stat: str, value: float, n: int, pcts: Dict[str, float], extra: Dict[str, Any]) -> Core:
    if value is None or not _num(value):
        raise NoResult(f"{metric}: no value")
    if not n or n < 1:
        raise NoResult(f"{metric}: no samples")
    return {
        "metric": metric,
        "value": value,
        "unit": unit,
        "stat": stat,
        "n": int(n),
        "percentiles": {k: v for k, v in pcts.items() if v is not None},
        "extra": numbers(extra),
    }


# ------------------------------------------------- timed phases (JSON lines)


def phases_from_dir(raw: str) -> Dict[str, List[dict]]:
    """`<phase>.jsonl` files of {"phase","name","ms","rc"} rows, by phase."""
    out: Dict[str, List[dict]] = {}
    for f in sorted(glob.glob(os.path.join(raw, "*.jsonl"))):
        rows = [r for r in read_jsonl(f) if "ms" in r]
        if rows:
            out[os.path.basename(f)[:-6]] = rows
    return out


def convert_phases(
    phases: Dict[str, List[dict]],
    primary: str,
    metric: str,
    extras: Dict[str, str],
    skip: Iterable[str] = ("prepare",),
) -> Tuple[Core, Samples]:
    """Timed CLI commands: the primary phase's successful samples give the
    value (p50) and percentiles; each other phase's p50 goes to `extra`."""
    ok = [r["ms"] for r in phases.get(primary, []) if r.get("rc") == 0]
    if not ok:
        raise NoResult(f"phase {primary}: no successful sample")
    extra: Dict[str, Any] = {"failed": sum(1 for r in phases[primary] if r.get("rc") != 0)}
    for phase, key in extras.items():
        v = [r["ms"] for r in phases.get(phase, []) if r.get("rc") == 0]
        if v:
            extra[key] = pct(v, 50)
    rows = []
    for phase in sorted(phases):
        if phase in skip:
            continue
        for r in phases[phase]:
            rows.append({"phase": phase, "name": r.get("name", ""), "ms": r["ms"], "rc": r.get("rc", "")})
    return core(metric, "ms", "p50", pct(ok, 50), len(ok), percentiles(ok), extra), indexed(rows)


# --------------------------------------------------------- spaces-startup


def convert_startup(raw: str, phase: str) -> Tuple[Core, Samples]:
    p = phases_from_dir(raw)
    if phase == "microvm":
        return convert_phases(
            p,
            "microvm-warm",
            "create_to_ready_ms",
            {
                "microvm-cold": "cold_p50_ms",
                "microvm-checkpoint": "checkpoint_p50_ms",
                "microvm-fork-checkpoint": "fork_checkpoint_p50_ms",
                "microvm-fork-running": "fork_running_p50_ms",
            },
        )
    if phase == "gvisor":
        return convert_phases(
            p,
            "gvisor-start",
            "create_to_ready_ms",
            {"gvisor-checkpoint": "checkpoint_p50_ms", "gvisor-fork-checkpoint": "fork_checkpoint_p50_ms"},
        )
    if phase == "minimal":
        return convert_phases(
            p,
            "minimal-warm",
            "create_to_http_ready_ms",
            {"minimal-cold": "cold_p50_ms", "minimal-gvisor": "gvisor_p50_ms"},
        )
    raise ValueError(f"unknown spaces-startup phase {phase}")


def convert_density(raw: str) -> Tuple[Core, Samples]:
    """`density-<kind>.jsonl`: one row per ramp step. The value is the most
    Spaces of the first kind that ran at once."""
    kinds = {}
    for f in sorted(glob.glob(os.path.join(raw, "density-*.jsonl"))):
        rows = [r for r in read_jsonl(f) if "running" in r]
        if rows:
            kinds[os.path.basename(f)[len("density-") : -6]] = rows
    if not kinds:
        raise NoResult("no density steps")
    order = [k for k in ("microvm", "gvisor") if k in kinds] + sorted(set(kinds) - {"microvm", "gvisor"})
    first = kinds[order[0]]
    last = first[-1]
    extra: Dict[str, Any] = {"mib_per_space": round(last["mem_used_mb"] / max(last["running"], 1), 1)}
    for k in order[1:]:
        lk = kinds[k][-1]
        extra[f"{k}_max_running"] = lk["running"]
        extra[f"{k}_mib_per_space"] = round(lk["mem_used_mb"] / max(lk["running"], 1), 1)
    rows = [
        {"kind": k, **{f: r.get(f, "") for f in ("started", "running", "batch_ms", "mem_available_mb", "mem_used_mb", "pss_mb")}}
        for k in order
        for r in kinds[k]
    ]
    c = core("max_running_spaces", "spaces", "max", max(r["running"] for r in first), len(first), {}, extra)
    return c, indexed(rows)


def convert_vm_density(path: str) -> Tuple[Core, Samples]:
    """`vm-density-<label>.jsonl`: host samples and exec probes per step, and
    a summary line. The value is the most VMs that ran at once."""
    rows = read_jsonl(path)
    summary = next((r for r in reversed(rows) if r.get("summary") and r.get("phase") == "vm-density"), None)
    if summary is None:
        raise NoResult(f"{path}: no vm-density summary line")
    probes = {r["running"]: r for r in rows if r.get("phase") == "probe"}
    hosts = [r for r in rows if r.get("phase") == "host" and r.get("vms")]
    if not hosts:
        raise NoResult(f"{path}: no host samples")
    samples = []
    for h in hosts:
        pr = probes.get(h["running"], {})
        samples.append(
            {
                "running": h["running"],
                "vms": h["vms"],
                "vm_rss_mb": h.get("vm_rss_mb", ""),
                "vm_private_p50_mb": h.get("vm_private_p50_mb", ""),
                "vm_shared_mb": h.get("vm_shared_mb", ""),
                "host_available_mb": h.get("host", {}).get("available_mb", ""),
                "host_pressure": h.get("host", {}).get("pressure", ""),
                "exec_p50_ms": pr.get("p50", ""),
                "exec_p95_ms": pr.get("p95", ""),
            }
        )
    fit = summary.get("fit_by_memory", {})
    extra = {
        "per_vm_mb": summary.get("per_vm_mb"),
        "vm_private_p50_mb": summary.get("vm_private_p50_mb"),
        "vm_shared_mb": summary.get("vm_shared_mb"),
        "exec_p95_ms_one": summary.get("exec_p95_ms_one"),
        "exec_p95_ms_max": summary.get("exec_p95_ms_max"),
        "vcpus": summary.get("vcpus"),
        "memory_mb": summary.get("memory_mb"),
        "fit_64g": fit.get("64g"),
        "fit_128g": fit.get("128g"),
        "fit_this_host": fit.get("this"),
    }
    c = core("max_running_vms", "vms", "max", summary["max_running"], len(samples), {}, extra)
    return c, indexed(samples)


# -------------------------------------------------------------- forks


def convert_microvm_fork(raw: str) -> Tuple[Core, Samples]:
    c, s = convert_phases(
        phases_from_dir(raw),
        "restore",
        "fork_from_checkpoint_ms",
        {"checkpoint": "checkpoint_p50_ms", "fork_running": "fork_running_p50_ms", "fanout": "fanout_wall_ms"},
        skip=("prepare", "fanout"),
    )
    mem = os.path.join(raw, "memory.json")
    if os.path.exists(mem):
        m = json.load(open(mem))
        c["extra"].update(numbers({"fanout_forks": m.get("forks"), "per_fork_mib": m.get("per_fork_mib")}))
    return c, s


def convert_fork_fanout(run: dict) -> Tuple[Core, Samples]:
    """run.sh's JSON: one entry per wave. The value is the p50 time to a
    ready fork in the largest wave."""
    waves = sorted(run.get("waves", []), key=lambda w: w.get("forks", 0))
    if not waves:
        raise NoResult("no fork waves")
    top = waves[-1]
    extra: Dict[str, Any] = {
        "forks": top["forks"],
        "wall_s": top.get("wall_s"),
        "private_mb_per_fork": round(top["private_mb_per_fork"], 1) if _num(top.get("private_mb_per_fork")) else None,
        "checkpoint_ms": run.get("checkpoint_ms"),
        "memory_mb": run.get("memory_mb"),
        "dirty_mb": run.get("dirty_mb"),
    }
    for w in waves[:-1]:
        extra[f"wall_s_{w['forks']}"] = w.get("wall_s")
        extra[f"fork_ms_p50_{w['forks']}"] = w.get("fork_ms_p50")
    fields = ("forks", "wall_s", "fork_ms_p50", "fork_ms_p95", "fork_ms_max", "private_mb_per_fork", "shared_file_mb")
    rows = [{f: w.get(f, "") for f in fields} for w in waves]
    pc = {"p50": top.get("fork_ms_p50"), "p95": top.get("fork_ms_p95"), "max": top.get("fork_ms_max")}
    return core("fork_ready_ms", "ms", "p50", top.get("fork_ms_p50"), top["forks"], pc, extra), indexed(rows)


def convert_fork_batch(res: dict) -> Tuple[Core, Samples]:
    """bench.py's JSON: each fork's own time on its machine (items)."""
    items = [i for i in res.get("items", []) if not i.get("error") and _num(i.get("fork_ms"))]
    extra = {
        "count": res.get("count"),
        "failed": res.get("failed", 0),
        "total_ms": res.get("total_ms"),
        "first_ready_ms": res.get("first_ready_ms"),
        "parallel": res.get("parallel"),
    }
    if items:
        v = [i["fork_ms"] for i in items]
        rows = [{f: i.get(f, "") for f in ("index", "host", "fork_ms", "elapsed_ms", "attempt")} for i in res["items"]]
        return core("fork_ms", "ms", "p50", pct(v, 50), len(v), percentiles(v), extra), indexed(rows)
    # A summary without its items (a committed baseline): its own p50/p95.
    if not res.get("ok"):
        raise NoResult("no ready fork")
    pc = {"p50": res.get("fork_ms_p50"), "p95": res.get("fork_ms_p95")}
    rows = [{k: res.get(k, "") for k in ("ok", "fork_ms_p50", "fork_ms_p95", "first_ready_ms", "total_ms")}]
    return core("fork_ms", "ms", "p50", res.get("fork_ms_p50"), res["ok"], pc, extra), indexed(rows)


# ------------------------------------------------------------ channel


def channel_metrics(doc: dict) -> Dict[str, dict]:
    """A channel_bench summary line ("results": [...]) or a committed
    baseline ("metrics": {...}), as metric -> fields."""
    if "results" in doc:
        return {r["metric"]: r for r in doc["results"] if "metric" in r}
    return {k: dict(v, metric=k) for k, v in doc.get("metrics", {}).items()}


def convert_channel(doc: dict) -> Tuple[Core, Samples]:
    m = channel_metrics(doc)
    ex = m.get("exec_run_ms")
    if not ex or not _num(ex.get("p50")):
        raise NoResult("no exec_run_ms")
    extra: Dict[str, Any] = {}
    for name, r in m.items():
        if name == "exec_run_ms":
            continue
        if name.endswith("_ms") and _num(r.get("p50")):
            extra[f"{name[:-3]}_p50_ms"] = r["p50"]
        for k in ("MiBps_p50", "MiBps"):
            if _num(r.get(k)):
                extra[f"{name.removesuffix('_ms')}_MiBps"] = r[k]
    rows = [{f: r.get(f, "") for f in ("metric", "n", "p50", "p95", "p99", "max", "MiBps_p50", "MiBps")} for r in m.values()]
    pc = {k: ex.get(k) for k in ("p50", "p95", "p99", "max")}
    # A baseline has no n; one call per iteration, ITERS of them.
    n = ex.get("n") or doc.get("iters")
    return core("exec_round_trip_ms", "ms", "p50", ex["p50"], n, pc, extra), indexed(rows)


# ----------------------------------------------------------- rollouts


def convert_rollouts(guard: dict, raw_rows: List[dict]) -> Tuple[Core, Samples]:
    """guard.sh's guard.json and the rollout_bench lines it kept
    (raw.jsonl). The value is a headless Space's create-to-first-exec p50."""
    creates = [r for r in raw_rows if r.get("cmd") == "create" and not r.get("summary") and "label" in r]
    serial = [r["ready_ms"] for r in creates if r.get("label") == "headless-serial" and r.get("ok") and _num(r.get("ready_ms"))]
    if not serial:
        raise NoResult("no headless-serial creates in raw.jsonl")
    rows = [{f: r.get(f, "") for f in ("label", "i", "create_ms", "first_exec_ms", "ready_ms", "ok")} for r in creates]
    extra = {k: v for k, v in guard.items() if k != "headless_create_p50_ms"}
    return core("create_to_first_exec_ms", "ms", "p50", pct(serial, 50), len(serial), percentiles(serial), extra), indexed(rows)


# ------------------------------------------------------- pool sessions


def convert_pool_sessions(jobs: List[dict]) -> Tuple[Core, Samples]:
    """`cua sb runs RUN --json` jobs: each session's Space start, run and
    delete times. The value is the p50 Space start."""
    ok = [j for j in jobs if j.get("state") == "succeeded"]
    starts = [j["create_ms"] for j in ok if _num(j.get("create_ms"))]
    if not starts:
        raise NoResult("no succeeded session with a start time")
    fin = sorted(j["finished_ms"] for j in ok if _num(j.get("finished_ms")))
    span_s = (fin[-1] - fin[0]) / 1000 if len(fin) > 1 else 0
    runs = [j["run_ms"] for j in ok if _num(j.get("run_ms"))]
    dels = [j["delete_ms"] for j in ok if _num(j.get("delete_ms"))]
    extra = {
        "sessions": len(jobs),
        "succeeded": len(ok),
        "failed": sum(1 for j in jobs if j.get("state") in ("error", "failed")),
        "run_p50_ms": pct(runs, 50),
        "delete_p50_ms": pct(dels, 50),
        "completed_per_hour": round(len(fin) / span_s * 3600) if span_s else None,
        "computers": len({j.get("node") for j in ok if j.get("node")}),
    }
    rows = [{f: j.get(f, "") for f in ("id", "state", "node", "create_ms", "run_ms", "delete_ms")} for j in jobs]
    return core("space_start_ms", "ms", "p50", pct(starts, 50), len(starts), percentiles(starts), extra), indexed(rows)


# ------------------------------------------------------------ dispatch


def _one(raw: str, pattern: str) -> str:
    found = sorted(glob.glob(os.path.join(raw, pattern)))
    if not found:
        raise NoResult(f"no {pattern} in {raw}")
    return found[-1]


def _json(path: str) -> dict:
    with open(path) as f:
        return json.load(f)


def _last_summary(path: str) -> dict:
    rows = [r for r in read_jsonl(path) if r.get("summary")]
    if not rows:
        raise NoResult(f"{path}: no summary line")
    return rows[-1]


CONVERTERS: Dict[str, Callable[[str], Tuple[Core, Samples]]] = {
    "spaces-startup.microvm": lambda raw: convert_startup(raw, "microvm"),
    "spaces-startup.gvisor": lambda raw: convert_startup(raw, "gvisor"),
    "spaces-startup.minimal": lambda raw: convert_startup(raw, "minimal"),
    "spaces-startup.density": convert_density,
    "spaces-startup.vm-density": lambda raw: convert_vm_density(_one(raw, "vm-density-*.jsonl")),
    "microvm-fork": convert_microvm_fork,
    "fork-fanout": lambda raw: convert_fork_fanout(_json(_one(raw, "fork-fanout.json"))),
    "fork-batch": lambda raw: convert_fork_batch(_json(_one(raw, "result.json"))),
    "channel": lambda raw: convert_channel(_last_summary(_one(raw, "channel.jsonl"))),
    "rollouts": lambda raw: convert_rollouts(
        _json(_one(raw, "guard.json")),
        read_jsonl(os.path.join(raw, "raw.jsonl")) if os.path.exists(os.path.join(raw, "raw.jsonl")) else [],
    ),
    "pool-sessions": lambda raw: convert_pool_sessions(_json(_one(raw, "jobs.json"))),
}


def convert(bench: str, raw: str) -> Tuple[Core, Samples]:
    """Native output in the directory `raw` -> (core fields, samples)."""
    try:
        fn = CONVERTERS[bench]
    except KeyError:
        raise ValueError(f"no converter for {bench}") from None
    return fn(raw)


# --------------------------------------------------------------- write

FIELDS = (
    "schema",
    "bench",
    "slug",
    "metric",
    "value",
    "unit",
    "stat",
    "n",
    "percentiles",
    "extra",
    "samples_csv",
    "measured_at",
    "build",
    "host",
    "environment",
    "method",
    "reproduce",
    "caveats",
    "status",
)


def finish(c: Core, **meta: Any) -> Dict[str, Any]:
    """A complete, ordered result: the converter's fields plus the run's
    metadata (bench, slug, measured_at, build, host, environment, method,
    reproduce, caveats)."""
    res = {"schema": SCHEMA, **c, **meta, "samples_csv": "samples.csv", "status": "measured"}
    res.setdefault("caveats", [])
    missing = [k for k in FIELDS if k not in res]
    if missing:
        raise ValueError(f"result is missing {missing}")
    return {k: res[k] for k in FIELDS}


def write(out_dir: str, result: Dict[str, Any], samples: Samples) -> None:
    os.makedirs(out_dir, exist_ok=True)
    cols: List[str] = []
    for r in samples:
        for k in r:
            if k not in cols:
                cols.append(k)
    if cols and cols[0] != "i":
        cols.remove("i")
        cols.insert(0, "i")
    with open(os.path.join(out_dir, result["samples_csv"]), "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=cols or ["i"], extrasaction="ignore")
        w.writeheader()
        for r in samples:
            w.writerow(r)
    with open(os.path.join(out_dir, "result.json"), "w") as f:
        json.dump(result, f, indent=2)
        f.write("\n")


def validate(result: Dict[str, Any]) -> List[str]:
    """The schema's rules for a measured result (what the site checks)."""
    errs = []
    if result.get("schema") != SCHEMA:
        errs.append("schema")
    if result.get("status") not in ("measured", "tbd"):
        errs.append("status")
    if result.get("status") == "measured":
        for k in ("value", "n", "measured_at", "build", "reproduce", "method", "host", "samples_csv", "stat"):
            if result.get(k) in (None, ""):
                errs.append(f"measured result needs {k}")
        if result.get("stat") not in ("p50", "mean", "max", "min", "count", "total"):
            errs.append("stat")
        if not isinstance(result.get("n"), int) or result["n"] < 1:
            errs.append("n")
        if not (result.get("build") or {}).get("source"):
            errs.append("build.source")
    for k, v in (result.get("extra") or {}).items():
        if not _num(v):
            errs.append(f"extra.{k} is not a number")
    for k, v in (result.get("percentiles") or {}).items():
        if not _num(v):
            errs.append(f"percentiles.{k} is not a number")
    return errs
