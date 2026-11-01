"""Compare two conformance runs (or a run and a committed fixture).

    python compare.py fixtures/e2b-2.42.0.json run.json [--rules rules.json]

Every check in the expected run must be in the actual run with the same
observations. `rules.json` lists the only allowed differences, each with a
reason. Exit status 1 on any other difference, with each one printed.
A check that fails differently, or a new or missing check, is a difference:
that is how SDK drift (new fields, changed shapes or errors) shows up.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def walk(expected, actual, path, rules, diffs):
    rule = rules.get(path)
    if rule is not None:
        mode = rule["mode"]
        if mode == "ignore":
            return
        if mode == "regex":
            if not (isinstance(actual, str) and re.fullmatch(rule["pattern"], actual, re.S)):
                diffs.append(f"{path}: {actual!r} does not match /{rule['pattern']}/")
            return
        if mode == "type":
            if type(expected) is not type(actual):
                diffs.append(f"{path}: type {type(actual).__name__} != {type(expected).__name__}")
            return
    if isinstance(expected, dict) and isinstance(actual, dict):
        for k in sorted(set(expected) | set(actual)):
            sub = f"{path}.{k}" if path else k
            if k not in actual:
                if rules.get(sub, {}).get("mode") != "ignore":
                    diffs.append(f"{sub}: missing (expected {json.dumps(expected[k])[:120]})")
                continue
            if k not in expected:
                if rules.get(sub, {}).get("mode") != "ignore":
                    diffs.append(f"{sub}: unexpected {json.dumps(actual[k])[:120]}")
                continue
            walk(expected[k], actual[k], sub, rules, diffs)
        return
    if isinstance(expected, list) and isinstance(actual, list):
        for i in range(max(len(expected), len(actual))):
            sub = f"{path}[{i}]"
            if i >= len(actual):
                diffs.append(f"{sub}: missing (expected {json.dumps(expected[i])[:120]})")
            elif i >= len(expected):
                diffs.append(f"{sub}: unexpected {json.dumps(actual[i])[:120]}")
            else:
                walk(expected[i], actual[i], sub, rules, diffs)
        return
    if expected != actual:
        diffs.append(f"{path}: {json.dumps(actual)[:160]} != expected {json.dumps(expected)[:160]}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("expected")
    ap.add_argument("actual")
    ap.add_argument(
        "--subset", action="store_true", help="only compare the cases present in the actual run"
    )
    ap.add_argument("--rules", default=str(HERE / "rules.json"))
    a = ap.parse_args()
    expected = json.loads(Path(a.expected).read_text())
    actual = json.loads(Path(a.actual).read_text())
    rules = (
        {r["path"]: r for r in json.loads(Path(a.rules).read_text())["rules"]} if a.rules else {}
    )
    diffs: list[str] = []
    if a.subset and "cases" in expected and "cases" in actual:
        expected = {
            **expected,
            "cases": {k: v for k, v in expected["cases"].items() if k in actual["cases"]},
        }
    walk(expected, actual, "", rules, diffs)
    if diffs:
        print(f"{len(diffs)} difference(s) from {a.expected}:")
        for d in diffs:
            print("  " + d)
        return 1
    print(f"conformant: {len(expected)} checks match {a.expected}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
