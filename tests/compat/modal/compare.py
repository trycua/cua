"""Compares a conformance run with the derived golden.

    compare.py fixtures/modal-1.6.2-derived.json run.json [--lane space|hermetic]

The golden lists, per case, what the client must observe, derived from the
client's sources (`derived_from`); a case missing from the run, failing, or
observing something else is a difference. Exit 1 on any.
"""

from __future__ import annotations

import json
import sys

args = [a for a in sys.argv[1:] if not a.startswith("--")]
golden = json.load(open(args[0]))
run = json.load(open(args[1]))
bad = 0
for name, g in sorted(golden["cases"].items()):
    r = run["cases"].get(name)
    if r is None:
        print(f"MISSING {name}")
        bad += 1
    elif r.get("skipped"):
        print(f"SKIPPED {name}: {r['skipped']}")
    elif not r.get("ok"):
        print(f"FAILED  {name}: {r.get('error')}")
        bad += 1
    elif r["observed"] != g["expect"]:
        print(
            f"DIFF    {name}\n  expected {json.dumps(g['expect'], sort_keys=True)}\n  observed {json.dumps(r['observed'], sort_keys=True)}"
        )
        bad += 1
    else:
        print(f"OK      {name}")
extra = sorted(set(run["cases"]) - set(golden["cases"]))
for name in extra:
    print(f"NEW     {name} (no golden)")
bad += len(extra)
print(
    f"{len(golden['cases']) - bad} of {len(golden['cases'])} match"
    if not bad
    else f"{bad} difference(s)"
)
sys.exit(1 if bad else 0)
