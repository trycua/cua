#!/usr/bin/env python3
"""Compares conformance runs with the committed golden and with each other.

    compare.py GOLDEN RUN.json [RUN.json ...] [--update] [--catalogue cases.json] [--needs mock,space]

Every run (python-sync, python-async, typescript) must pass its cases and its
observations must equal the golden's, case by case. With --catalogue, each run
must also contain every catalogued case that its `needs` lane and runner cover
(a case missing from one language is drift). --update rewrites the golden
from the first run (python-sync) after the checks that do not involve it.

The golden is "derived": each case asserts, in code, the behaviour the SDK
sources define (cases.json `ref`), and the golden stores the observation the
asserting cases returned, so the other runners and later SDK versions are held
to the same values. No Daytona service was ever called to produce it.
"""

from __future__ import annotations

import argparse
import json
import sys


def load(p):
    return json.load(open(p))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("golden")
    ap.add_argument("runs", nargs="+")
    ap.add_argument("--update", action="store_true")
    ap.add_argument("--catalogue")
    ap.add_argument("--needs", default="mock,space")
    ap.add_argument(
        "--allow-new", action="store_true", help="cases absent from the golden are not failures"
    )
    a = ap.parse_args()
    runs = [load(r) for r in a.runs]
    needs = set(a.needs.split(","))
    problems: list[str] = []
    try:
        golden = load(a.golden)
    except FileNotFoundError:
        golden = {"label": "derived", "cases": {}}
    gcases = golden.get("cases", {})
    cat = {c["id"]: c for c in load(a.catalogue)} if a.catalogue else {}
    ran_any: set[str] = set()
    for run in runs:
        name = run["runner"]
        for cid, res in sorted(run["cases"].items()):
            ran_any.add(cid)
            if not res["ok"]:
                problems.append(f"{name}: {cid} failed: {res.get('error')}")
                continue
            if res.get("skipped"):
                print(f"SKIP {name}: {cid}: {res['skipped']}")
                continue
            if cid in gcases:
                if gcases[cid] != res["obs"] and not a.update:
                    problems.append(
                        f"{name}: {cid} observation differs from the golden\n    golden:   {json.dumps(gcases[cid], sort_keys=True)[:400]}\n    observed: {json.dumps(res['obs'], sort_keys=True)[:400]}"
                    )
            elif not a.allow_new and not a.update:
                problems.append(
                    f"{name}: {cid} has no golden entry (run with --update after reviewing)"
                )
        for cid, c in cat.items():
            if c["needs"] not in needs:
                continue
            modes = c.get("runners", ["python-sync", "python-async", "typescript"])
            if name in modes and cid not in run["cases"]:
                problems.append(
                    f"{name}: catalogued case {cid} was not run (missing in this runner)"
                )
    if a.update:
        first = runs[0]
        for run in runs:  # the first run wins; a case it skipped comes from the next
            for cid, res in run["cases"].items():
                if res["ok"] and not res.get("skipped") and (cid not in gcases or run is first):
                    gcases[cid] = res["obs"]
        golden = {
            "label": "derived",
            "sdk": first["sdk"],
            "note": "Observations of cases whose assertions encode the SDK-defined behaviour; not recorded from a Daytona service.",
            "cases": dict(sorted(gcases.items())),
        }
        import os

        os.makedirs(os.path.dirname(os.path.abspath(a.golden)), exist_ok=True)
        json.dump(golden, open(a.golden, "w"), indent=1, sort_keys=True)
        open(a.golden, "a").write("\n")
    for p in problems:
        print("DRIFT " + p)
    total = sum(len(r["cases"]) for r in runs)
    print(f"{total} case runs over {len(runs)} runners, {len(problems)} problems")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
