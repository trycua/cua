#!/usr/bin/env python3
"""Builds cases.json from the Python registry and checks the TypeScript one matches.

catalogue.py [--write]   (needs the pinned `daytona` and `npm ci` in ts/)
"""

import json
import os
import subprocess
import sys

here = os.path.dirname(os.path.abspath(__file__))
py = json.loads(
    subprocess.check_output([sys.executable, f"{here}/py/harness.py", "--list"], text=True)
)
ts = json.loads(
    subprocess.check_output(["npx", "tsx", "runner.ts", "--list"], cwd=f"{here}/ts", text=True)
)
pyids = {c["id"]: c for c in py}
tsids = {c["id"]: c for c in ts}
problems = []
for i in sorted(set(pyids) | set(tsids)):
    if i not in tsids:
        problems.append(f"{i}: missing in TypeScript")
    elif i not in pyids:
        problems.append(f"{i}: missing in Python")
    elif pyids[i]["needs"] != tsids[i]["needs"]:
        problems.append(f"{i}: needs differs ({pyids[i]['needs']} vs {tsids[i]['needs']})")
cat = []
for c in py:
    runners = ["python-sync", "python-async", "typescript"]
    runners = [
        r
        for r in runners
        if not (r == "python-sync" and "sync" not in c["modes"])
        and not (r == "python-async" and "async" not in c["modes"])
    ]
    cat.append({"id": c["id"], "needs": c["needs"], "ref": c["ref"], "runners": runners})
if "--write" in sys.argv:
    json.dump(cat, open(f"{here}/cases.json", "w"), indent=1)
    open(f"{here}/cases.json", "a").write("\n")
elif json.load(open(f"{here}/cases.json")) != cat:
    problems.append("cases.json is stale: run catalogue.py --write")
for p in problems:
    print("DRIFT", p)
print(f"{len(cat)} cases, {len(problems)} problems")
sys.exit(1 if problems else 0)
