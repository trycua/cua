#!/usr/bin/env python3
"""Compares the gRPC surface of a newer `modal` client with the pinned one.

    drift.py OLD_SURFACE.json NEW_SURFACE.json [--served served-rpcs.txt]

Both files come from `extract_descriptors.py` (`surface-<version>.json`).
What the gateway serves is `served-rpcs.txt`; a change matters when it
touches an RPC the gateway serves, or a message reachable from one:

- FAIL: a served RPC was removed or its request/response type or streaming
  changed; a field of a reachable message was removed, renumbered or changed
  type; an enum value of a reachable enum was removed or renumbered.
- NOTE: new RPCs (listed, so someone decides whether to serve them), new
  fields and values of reachable messages (proto3: old servers ignore them),
  changes only to RPCs the gateway does not serve.

Exit 1 on any FAIL.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path


def rpcs(surface: dict) -> dict[str, dict]:
    return {f"{svc}/{m}": d for svc, ms in surface["services"].items() for m, d in ms.items()}


def reachable(surface: dict, roots: set[str]) -> set[str]:
    seen: set[str] = set()
    todo = list(roots)
    msgs = surface["messages"]
    while todo:
        m = todo.pop()
        if m in seen or m not in msgs:
            continue
        seen.add(m)
        for _num, _type, _rep, ref in msgs[m].values():
            if ref:
                todo.append(ref)
    return seen


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    served_path = Path(sys.argv[sys.argv.index("--served") + 1]) if "--served" in sys.argv else None
    if served_path and str(served_path) in args:
        args.remove(str(served_path))
    old = json.loads(Path(args[0]).read_text())
    new = json.loads(Path(args[1]).read_text())
    served = set(served_path.read_text().split()) if served_path else set()
    fails: list[str] = []
    notes: list[str] = []

    a, b = rpcs(old), rpcs(new)
    for name in sorted(set(a) - set(b)):
        (fails if name in served else notes).append(f"RPC removed: {name}")
    for name in sorted(set(b) - set(a)):
        notes.append(f"RPC added: {name} ({'stream' if b[name]['server_streaming'] else 'unary'})")
    for name in sorted(set(a) & set(b)):
        if a[name] != b[name]:
            msg = f"RPC changed: {name}: {a[name]} -> {b[name]}"
            (fails if name in served else notes).append(msg)

    roots: set[str] = set()
    for name in served:
        for s in (a, b):
            if name in s:
                roots.add(s[name]["in"])
                roots.add(s[name]["out"])
    reach = reachable(old, roots) | reachable(new, roots)
    om, nm = old["messages"], new["messages"]
    for m in sorted(reach):
        if m not in om or m not in nm:
            (fails if m in om else notes).append(f"message {'removed' if m in om else 'added'}: {m}")
            continue
        for f in sorted(set(om[m]) | set(nm[m])):
            if f not in nm[m]:
                fails.append(f"field removed: {m}.{f} {om[m][f]}")
            elif f not in om[m]:
                notes.append(f"field added: {m}.{f} {nm[m][f]}")
            elif om[m][f] != nm[m][f]:
                fails.append(f"field changed: {m}.{f}: {om[m][f]} -> {nm[m][f]}")
    oe, ne = old["enums"], new["enums"]
    for e in sorted(set(oe) & set(ne)):
        removed = {v for v in oe[e] if v not in ne[e]}
        renumbered = {v for v in oe[e] if v in ne[e] and oe[e][v] != ne[e][v]}
        # Enums are checked when a reachable message uses them.
        used = any(ref == e for m in reach for (_n, _t, _r, ref) in nm.get(m, {}).values())
        if used and (removed or renumbered):
            fails.append(f"enum {e}: removed {sorted(removed)} renumbered {sorted(renumbered)}")
        for v in sorted(set(ne[e]) - set(oe[e])):
            if used:
                notes.append(f"enum value added: {e}.{v}")

    print(f"modal {old.get('modal_version')} -> {new.get('modal_version')}: "
          f"{len(a)} -> {len(b)} RPCs, {len(fails)} breaking change(s), {len(notes)} note(s)")
    for f in fails:
        print("FAIL " + f)
    for n in notes:
        print("note " + n)
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
