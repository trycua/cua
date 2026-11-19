"""Merges the cases of a partial run into a recording (after re-recording a few cases).

python merge_cases.py fixtures/cases-py-2.56.0.json partial.json [--drop name,...]
"""

import json
import sys

dst, src = sys.argv[1], sys.argv[2]
drop = sys.argv[4].split(",") if len(sys.argv) > 4 and sys.argv[3] == "--drop" else []
d = json.load(open(dst))
p = json.load(open(src))
d["cases"].update(p["cases"])
for n in drop:
    d["cases"].pop(n, None)
json.dump(d, open(dst, "w"), indent=1, sort_keys=True, ensure_ascii=False)
print(f"{dst}: {len(d['cases'])} cases ({len(p['cases'])} replaced)")
