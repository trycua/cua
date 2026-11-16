#!/usr/bin/env python3
"""Refresh or verify every artifact digest in the CDB-S03 manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

BUNDLE = Path(__file__).resolve().parent.parent
MANIFEST = BUNDLE / "task.cuabench.json"


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def entries(document: dict):
    for variant in document["variants"]:
        yield from variant["fixture_artifacts"]
    yield from document["evaluator"]["inputs"]
    for fixture in document["reset"]["failure_fixtures"]:
        yield fixture["artifact"]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    document = json.loads(MANIFEST.read_text(encoding="utf-8"))
    stale = []
    for entry in entries(document):
        path = BUNDLE / entry["path"]
        if not path.is_file():
            raise SystemExit(f"missing artifact: {entry['path']}")
        expected = digest(path)
        if entry["sha256"] != expected:
            stale.append(entry["path"])
            entry["sha256"] = expected
    if args.check:
        if stale:
            print("stale digests: " + ", ".join(stale))
            return 1
        print("digests up to date")
        return 0
    MANIFEST.write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"updated {len(stale)} digest(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
