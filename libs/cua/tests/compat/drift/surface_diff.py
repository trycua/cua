#!/usr/bin/env python3
"""Readable diff of two public-surface dumps (surface_py.py / surface_ts.mjs).

    surface_diff.py fixture.json current.json [--allow-added]

Prints removed, changed and added symbols/members, one line each, and exits 1
when anything differs (added members too unless --allow-added: a new SDK
call needs a decision on the gateway's coverage matrix)."""

from __future__ import annotations

import json
import sys


def flat(d, prefix=""):
    out = {}
    if isinstance(d, dict):
        for k, v in d.items():
            out.update(flat(v, f"{prefix}{k}." if isinstance(v, dict) else f"{prefix}{k}"))
            if not isinstance(v, dict):
                out[f"{prefix}{k}"] = v
    else:
        out[prefix.rstrip(".")] = d
    return out


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    allow_added = "--allow-added" in sys.argv
    a, b = (json.load(open(p)) for p in args[:2])
    fa, fb = flat(a["symbols"]), flat(b["symbols"])
    removed = sorted(k for k in fa if k not in fb)
    added = sorted(k for k in fb if k not in fa)
    changed = sorted(k for k in fa if k in fb and fa[k] != fb[k])
    print(f"{a.get('package')} {a.get('version')} (fixture) vs {b.get('version')} (current)")
    for k in removed:
        print(f"  REMOVED  {k}")
    for k in changed:
        print(
            f"  CHANGED  {k}\n      was: {json.dumps(fa[k])[:300]}\n      now: {json.dumps(fb[k])[:300]}"
        )
    for k in added:
        print(f"  ADDED    {k}")
    bad = removed or changed or (added and not allow_added)
    print(f"{len(removed)} removed, {len(changed)} changed, {len(added)} added")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
