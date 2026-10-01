#!/usr/bin/env python3
"""Re-apply the scrub rules to casts that are already on disk.

``record.py`` scrubs as it records, so a fresh cast is already clean. This
exists for two cases the recorder cannot cover: a rule added after a cast was
taken, and a cast that came from somewhere else. It rewrites in place and
prints what it changed, so the diff is reviewable.

    python3 scrub.py path/to/*.cast
"""

from __future__ import annotations

import json
import sys

from record import _SCRUB_RULES, scrub


def scrub_file(path: str) -> int:
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().splitlines()
    if not lines:
        return 0

    header = json.loads(lines[0])
    changed = 0
    out = []
    for line in lines[1:]:
        event = json.loads(line)
        before = event[2]
        after = scrub(before.encode(), None).decode("utf-8", "replace")
        if after != before:
            changed += 1
            event[2] = after
        out.append(json.dumps(event, ensure_ascii=False))

    known = set(header.get("cua_scrubbed", []))
    for pattern, _ in _SCRUB_RULES:
        known.add(pattern.pattern.decode("utf-8", "replace"))
    known.add("$HOME -> /Users/operator")
    header["cua_scrubbed"] = sorted(known)

    with open(path, "w", encoding="utf-8") as fh:
        fh.write(json.dumps(header) + "\n")
        for line in out:
            fh.write(line + "\n")
    return changed


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return 2
    for path in argv:
        changed = scrub_file(path)
        print(f"{path}: {changed} event(s) rewritten")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
