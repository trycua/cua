#!/usr/bin/env python3
"""Time-balanced shards for the cua SDK e2e suite (stdlib only).

The PR workflow runs the suite on N runners. Every shard collects the same
list of units, weighs each one from timings.json and runs the units the plan
gives it, so the shards are disjoint and together cover every unit without any
coordination:

    py    one pytest test (node id; weighed by test name)
    ts    one tests/e2e/cua-sdk/typescript/*.test.mjs file
    rust  one prebuilt integration-test binary of tests/e2e/cua-sdk/rust

A unit missing from timings.json (a new test) gets the language's default
weight, so it still runs on exactly one shard.

The plan is greedy longest-processing-time: heaviest unit first, onto the
shard with the least work so far, ties broken by name. With ~120 units and the
heaviest at about a third of a shard that lands within 1% of even.

Refresh timings.json from the `cua-e2e-shard-*` artifacts of a green run:

    for i in 1 2 3 4; do gh run download RUN -n cua-e2e-shard-$i -D art/$i; done
    python3 tests/e2e/cua-sdk/sharding.py timings art/* >tests/e2e/cua-sdk/timings.json
    python3 tests/e2e/cua-sdk/sharding.py check art/* --shards 4

`check` prints each shard's predicted and measured seconds and the spread, and
fails when the shards together do not cover the plan (a shard missing, a unit
dropped or planned twice).
"""

from __future__ import annotations

import json
import statistics
import sys
from pathlib import Path

TIMINGS = Path(__file__).with_name("timings.json")
LANGS = ("py", "ts", "rust")
Unit = tuple[str, str]  # (lang, unit)


def load_timings(path: Path = TIMINGS) -> dict:
    return json.loads(Path(path).read_text()) if Path(path).exists() else {}


def timing_key(lang: str, unit: str) -> str:
    """py units are pytest node ids, weighed by the test's own name (what the
    results record): the part after `file.py::`."""
    return unit.split("::", 1)[1] if lang == "py" and "::" in unit else unit


def weight(timings: dict, lang: str, unit: str) -> float:
    w = timings.get(lang, {}).get(timing_key(lang, unit))
    if w is None:
        w = timings.get("default", {}).get(lang, 1.0)
    return float(w)


def parse_shard(spec: str) -> tuple[int, int]:
    """`2/4` -> (2, 4), 1-based."""
    try:
        i, n = (int(x) for x in spec.split("/"))
    except ValueError:
        raise SystemExit(f"--shard {spec!r}: want I/N, for example 2/4") from None
    if not 1 <= i <= n:
        raise SystemExit(f"--shard {spec!r}: I must be 1..N")
    return i, n


def assign(units: list[Unit], n: int, timings: dict) -> list[list[Unit]]:
    """Split units into n shards by weight (longest processing time first)."""
    if n < 1:
        raise ValueError("need at least one shard")
    load = [0.0] * n
    shards: list[list[Unit]] = [[] for _ in range(n)]
    for lang, unit in sorted(set(units), key=lambda u: (-weight(timings, *u), u)):
        i = min(range(n), key=lambda k: (load[k], k))
        shards[i].append((lang, unit))
        load[i] += weight(timings, lang, unit)
    return shards


def predicted(shard: list[Unit], timings: dict) -> float:
    return sum(weight(timings, *u) for u in shard)


def spread(values: list[float]) -> float:
    """(max - min) / max, as a percentage."""
    top = max(values, default=0.0)
    return 0.0 if top <= 0 else 100.0 * (top - min(values)) / top


def _jsonl(path: Path) -> list[dict]:
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def measured(dirs: list[Path]) -> dict:
    """Per-unit seconds from shard result dirs: py from the per-test results,
    ts and rust from the wall time each file or binary took (shard.json)."""
    py: dict[str, list[float]] = {}
    other: dict[str, dict[str, list[float]]] = {"ts": {}, "rust": {}}
    for d in dirs:
        for r in _jsonl(d / "py.jsonl"):
            py.setdefault(r["test"], []).append(float(r.get("secs", 0)))
        meta = d / "shard.json"
        if meta.exists():
            for key, secs in json.loads(meta.read_text()).get("unit_secs", {}).items():
                lang, unit = key.split(":", 1)
                other.setdefault(lang, {}).setdefault(unit, []).append(float(secs))
    out: dict = {"py": {k: round(statistics.median(v), 1) for k, v in sorted(py.items())}}
    for lang in ("ts", "rust"):
        out[lang] = {k: round(statistics.median(v), 1) for k, v in sorted(other[lang].items())}
    return out


def build_timings(dirs: list[Path]) -> dict:
    t = measured(dirs)
    t["default"] = {
        lang: round(statistics.median(t[lang].values()), 1) if t[lang] else 1.0 for lang in LANGS
    }
    return t


def main(argv: list[str]) -> int:
    if len(argv) < 2 or argv[0] not in ("timings", "check"):
        print(__doc__)
        return 2
    cmd, rest = argv[0], argv[1:]
    n = 0
    if "--shards" in rest:
        k = rest.index("--shards")
        n = int(rest[k + 1])
        del rest[k : k + 2]
    dirs = [Path(a) for a in rest]
    if cmd == "timings":
        print(json.dumps(build_timings(dirs), indent=1, sort_keys=True))
        return 0
    metas = sorted(
        (json.loads((d / "shard.json").read_text()) for d in dirs), key=lambda m: m["shard"]
    )
    for m in metas:
        settings = ", ".join(f"{k}={v}" for k, v in m.get("settings", {}).items())
        print(
            f"shard {m['shard']}: predicted {m['predicted_secs']:6.1f}s, "
            f"measured {m['wall_secs']:6.1f}s ({settings})"
        )
    print(
        f"spread: predicted {spread([m['predicted_secs'] for m in metas]):.1f}%, "
        f"measured {spread([m['wall_secs'] for m in metas]):.1f}%"
    )
    problems = coverage_problems(metas, n)
    if problems:
        print("\n".join(["", "PROBLEMS:", *problems]))
        return 1
    return 0


def coverage_problems(metas: list[dict], n: int = 0) -> list[str]:
    """The shards together ran the plan: one shard of each index, one plan,
    every unit on exactly one shard."""
    problems = []
    count = n or (metas[0]["of"] if metas else 0)
    if sorted(m["shard"] for m in metas) != list(range(1, count + 1)):
        problems.append(f"want shards 1..{count}, found {sorted(m['shard'] for m in metas)}")
    if {m["of"] for m in metas} - {count}:
        problems.append(f"shards disagree on the shard count: {sorted({m['of'] for m in metas})}")
    totals = {m["total_units"] for m in metas}
    if len(totals) != 1:
        problems.append(f"shards planned different suites: {sorted(totals)} units")
    ran = [(lang, u) for m in metas for lang, units in m["units"].items() for u in units]
    if len(set(ran)) != len(ran):
        problems.append("a unit is planned on more than one shard")
    if totals and len(ran) != next(iter(totals)):
        problems.append(f"{len(ran)} units planned across shards, the suite has {sorted(totals)}")
    return problems


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
