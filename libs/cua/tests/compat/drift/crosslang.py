#!/usr/bin/env python3
"""The Python and TypeScript runners of a provider's shared cases must see the
same thing. Given two runs (or two committed fixtures) of the same case file,
for every case and step both ran: the op is the same, ok/raised agree, and
primitive results (strings, numbers, booleans, null) are equal. Exception
class names and object shapes legitimately differ between SDKs and are not
compared (each language's run is compared with its own fixture by the
provider's compare.py).

    crosslang.py py-run.json ts-run.json [--allow crosslang-allow.json]

crosslang-allow.json: {"<case>.<step>": "reason"} for differences that are
real SDK differences, each with a reason. Exit 1 on any other difference."""

from __future__ import annotations

import json
import sys
from pathlib import Path


def prim(x):
    return x is None or isinstance(x, (str, int, float, bool))


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    allow = {}
    if "--allow" in sys.argv:
        allow = json.loads(Path(sys.argv[sys.argv.index("--allow") + 1]).read_text())
        args = [a for a in args if a != sys.argv[sys.argv.index("--allow") + 1]]
    py, ts = (json.load(open(p))["cases"] for p in args[:2])
    shared = sorted(set(py) & set(ts))
    diffs, seen = [], 0
    for c in shared:
        sp, st = py[c].get("steps", {}), ts[c].get("steps", {})
        for s in sorted(set(sp) & set(st)):
            seen += 1
            a, b = sp[s], st[s]
            key = f"{c}.{s}"
            msg = None
            if a.get("op") != b.get("op"):
                msg = f"op {a.get('op')} (py) vs {b.get('op')} (ts)"
            elif ("raised" in a) != ("raised" in b):
                msg = f"{'raised' if 'raised' in a else 'ok'} (py) vs {'raised' if 'raised' in b else 'ok'} (ts)"
            elif "ok" in a and prim(a["ok"]) and prim(b.get("ok")) and a["ok"] != b["ok"]:
                msg = f"result {a['ok']!r} (py) vs {b['ok']!r} (ts)"
            if msg and key not in allow:
                diffs.append(f"{key}: {msg}")
    only_py, only_ts = sorted(set(py) - set(ts)), sorted(set(ts) - set(py))
    print(
        f"{len(shared)} shared cases, {seen} steps compared; only in py: {only_py}; only in ts: {only_ts}"
    )
    for d in diffs:
        print("  DIFF", d)
    stale = [
        k for k in allow if k not in {f"{c}.{s}" for c in shared for s in py[c].get("steps", {})}
    ]
    for k in stale:
        print("  STALE allow entry", k)
    return 1 if diffs or stale else 0


if __name__ == "__main__":
    sys.exit(main())
