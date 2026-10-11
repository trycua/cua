#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Check that pins.json `cdb_pack.tree_sha256` matches the task pack in this repository.

    tools/check_pack_pins.py [--write]

The preflight refuses a pack task whose tree digest differs from its pin; this check catches a task
change that was committed without updating the pin. `--write` updates the pins instead.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import cdb_adapter  # noqa: E402

PINS = HERE / "pins.json"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--write", action="store_true", help="update pins.json instead of checking")
    args = parser.parse_args()
    pins = json.loads(PINS.read_text("utf-8"))
    root = cdb_adapter.REPO_TASKPACK / "tasks"
    want = pins["cdb_pack"]["tree_sha256"]
    got = {task: cdb_adapter.tree_sha256(root / task) for task in sorted(want)}
    if args.write:
        pins["cdb_pack"]["tree_sha256"] = got
        PINS.write_text(json.dumps(pins, indent=2) + "\n", "utf-8")
        print(json.dumps(got, indent=2))
        return 0
    bad = [task for task in want if got[task] != want[task]]
    for task in sorted(want):
        print(f"{'ok  ' if task not in bad else 'FAIL'} {task} {got[task]}")
    return 1 if bad else 0


if __name__ == "__main__":
    raise SystemExit(main())
