"""Rewrites cases/*.json with one step per line (stable diffs).

python fmt_cases.py cases/10-commands.json ...
"""

import json
import sys


def line(x):
    return json.dumps(x, ensure_ascii=False, separators=(", ", ": "))


for path in sys.argv[1:]:
    d = json.load(open(path))
    out = []
    for c in d["cases"]:
        head = {k: v for k, v in c.items() if k != "steps"}
        h = line(head)[:-1]
        steps = ",\n ".join(line(s) for s in c["steps"])
        out.append(f'{h}, "steps": [\n {steps}\n]}}')
    extra = {k: v for k, v in d.items() if k != "cases"}
    pre = "".join(f'"{k}": {line(v)}, ' for k, v in extra.items())
    open(path, "w").write("{" + pre + '"cases": [\n' + ",\n".join(out) + "\n]}\n")
