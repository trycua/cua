#!/usr/bin/env python3
"""Verify a freshly reset CDB-S03 workspace byte for byte."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
FIXTURE = BUNDLE / "fixture"
FACTS = {
    "task": "cua.shared.cdb-s03.invoice-reconciliation",
    "mail_store": "mail",
    "workbook": "planning/kestrel-wharf-reconciliation.ods",
    "book": "book/kestrel-wharf.gnucash",
}


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def tree(root: Path) -> dict[str, str]:
    if not root.is_dir():
        return {}
    return {
        path.relative_to(root).as_posix(): digest(path)
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def verify(workspace: Path) -> dict:
    expected = tree(FIXTURE)
    actual = {
        path: value
        for path, value in tree(workspace).items()
        if path != "cdb-s03.workspace.json" and not path.startswith(".runtime/")
    }
    try:
        facts = json.loads((workspace / "cdb-s03.workspace.json").read_text(encoding="utf-8"))
    except (OSError, ValueError, TypeError):
        facts = None
    missing = sorted(set(expected) - set(actual))
    extra = sorted(set(actual) - set(expected))
    changed = sorted(path for path in set(expected) & set(actual) if expected[path] != actual[path])
    ok = not missing and not extra and not changed and facts == FACTS
    return {
        "ok": ok,
        "workspace": str(workspace.resolve()),
        "missing": missing,
        "extra": extra,
        "changed": changed,
        "facts_valid": facts == FACTS,
        "files_checked": len(expected),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", type=Path, required=True)
    args = parser.parse_args()
    report = verify(args.workspace.resolve())
    sys.stdout.write(json.dumps(report, sort_keys=True) + "\n")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
