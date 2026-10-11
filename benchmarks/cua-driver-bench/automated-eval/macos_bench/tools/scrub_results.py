#!/usr/bin/env python3
"""Public copy of a results.jsonl: CDB rows lose everything that could disclose the private task pack.

The CDB suite is proprietary (../../PROVENANCE.md). For rows of tasks with `"kind": "cdb"` this drops the check
names, the agent's final text, the evaluator diagnostics and free-text notes, and keeps counts of checks passed
and failed. Probe rows (BenchLab, MIT) are copied unchanged.

  scrub_results.py RESULTS.jsonl OUT.jsonl
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

DROP = ("checks", "final_text", "diagnostics", "notes", "evaluator_error", "infra_message", "init")


def scrub(row: dict) -> dict:
    if not str(row.get("task", "")).startswith("CDB-"):
        return row
    out = {k: v for k, v in row.items() if k not in DROP}
    checks = row.get("checks") or {}
    out["checks_passed"] = sum(1 for v in checks.values() if v)
    out["checks_failed"] = sum(1 for v in checks.values() if not v)
    return out


def main() -> int:
    src, dst = Path(sys.argv[1]), Path(sys.argv[2])
    rows = [json.loads(line) for line in src.read_text("utf-8").splitlines() if line.strip()]
    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.write_text("".join(json.dumps(scrub(r), sort_keys=True) + "\n" for r in rows), "utf-8")
    print(f"{len(rows)} rows -> {dst}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
